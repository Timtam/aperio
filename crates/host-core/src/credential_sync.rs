//! The single chokepoint for cross-device credential sync.
//!
//! Account secrets (CalDAV / WebDAV passwords, OAuth refresh tokens, API
//! tokens) only ever leave the device through the sync log when
//! **end-to-end encryption is enabled** — so a secret always lives inside
//! an encrypted log blob, never plaintext on the remote. This module is
//! the *only* place that turns a stored secret into a `credential.*`
//! event, so the E2E gate lives in exactly one auditable spot.
//!
//! The gate is doubled up on purpose:
//!
//!   1. **E2E must be on.** Checked against `PREF_E2E_ENABLED`, the local
//!      mirror of `meta.json.e2e_enabled` (the same flag
//!      `build_adapter_from_prefs` uses to decide whether to wrap the
//!      adapter in `EncryptingAdapter` on the next push). When it's off,
//!      nothing is emitted — credentials stay device-local.
//!   2. **Slot must be syncable.** Only `password` / `refresh_token` /
//!      `api_token` may travel; the short-lived `access_token` (re-derived
//!      per device) and the E2E key itself are refused even if a caller
//!      passes them. (See [`SecretSlot::syncable_from_wire`].)
//!
//! The matching purge side — making sure these events never survive an
//! E2E *downgrade* — lives in the `disable_sync_encryption` flow.

use sync_core::{
    CredentialPayload, CredentialSlotPayload, EventEnvelope, LogFile, Snapshot, SyncEvent,
    SyncResult,
};
use sync_engine::{EventLogWriter, SecretSlot, SecretStore};

use crate::accounts::AccountsRepo;
use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// User-prefs key mirroring `meta.json.e2e_enabled` — the local flag that
/// decides whether the sync adapter wraps in `EncryptingAdapter` and whether
/// credentials may enter the sync log. Owned here (the single auditable home
/// for the credential-sync gate); `commands::sync` re-exports it.
pub const PREF_E2E_ENABLED: &str = "sync.adapter.e2eEnabled";

/// Whether end-to-end encryption is currently on for the sync dataset —
/// the gate that decides if a credential may enter the sync log at all.
pub fn e2e_enabled(conn: &SharedConn) -> bool {
    UserPrefsRepo::new(conn)
        .get(PREF_E2E_ENABLED)
        .ok()
        .flatten()
        .as_deref()
        == Some("true")
}

/// Emit a `credential.set` event for one account secret — but ONLY when
/// E2E is on *and* the slot is on the syncable allowlist. Otherwise it is
/// a no-op and the secret stays device-local (keychain only). Call this
/// right after a successful secret-store write.
pub fn emit_credential_set(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    account_id: &str,
    slot: SecretSlot,
    secret: &str,
) {
    if account_id == "local" {
        return;
    }
    // Defense in depth: refuse non-syncable slots (access_token / the E2E
    // key) before the secret can ever reach an event.
    if SecretSlot::syncable_from_wire(slot.wire_name()).is_none() {
        return;
    }
    if !e2e_enabled(conn) {
        return;
    }
    event_log.append(SyncEvent::CredentialSet(CredentialPayload {
        account_id: account_id.to_string(),
        slot: slot.wire_name().to_string(),
        secret: secret.to_string(),
    }));
}

/// Emit a `credential.cleared` event for one slot (same gating as
/// [`emit_credential_set`]). Use when a single secret slot is removed
/// without deleting the whole account (account deletion already cascades
/// via `account.deleted`).
pub fn emit_credential_cleared(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    account_id: &str,
    slot: SecretSlot,
) {
    if account_id == "local" {
        return;
    }
    if SecretSlot::syncable_from_wire(slot.wire_name()).is_none() {
        return;
    }
    if !e2e_enabled(conn) {
        return;
    }
    event_log.append(SyncEvent::CredentialCleared(CredentialSlotPayload {
        account_id: account_id.to_string(),
        slot: slot.wire_name().to_string(),
    }));
}

/// Push every local account secret into the (now-encrypted) sync log —
/// used when E2E is turned on *after* accounts already exist, so the
/// user's other devices pick them up without re-entering. Each secret
/// routes through [`emit_credential_set`], so this is a no-op unless E2E
/// is actually on and only the syncable slots are ever touched. Best
/// effort: a missing slot is normal (not every account has every slot)
/// and a keychain read error for one slot doesn't abort the rest.
pub fn emit_all_local_credentials(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    secrets: &dyn SecretStore,
) {
    let accounts = match AccountsRepo::new(conn).list() {
        Ok(accounts) => accounts,
        Err(err) => {
            tracing::warn!(?err, "credential bulk-emit: failed to list accounts");
            return;
        }
    };
    for account in accounts {
        for slot in [
            SecretSlot::Password,
            SecretSlot::RefreshToken,
            SecretSlot::ApiToken,
        ] {
            // Read through the injected platform secret store (the desktop
            // keyring; the mobile keychain bridge) — never a hard-coded backend.
            if let Ok(secret) = secrets.retrieve(&account.id, slot) {
                emit_credential_set(event_log, conn, &account.id, slot, &secret);
            }
        }
    }
}

/// Remove every `credential.*` event from a log file, returning a rebuilt
/// log under the same name (device + timestamp) so it overwrites the same
/// remote path.
///
/// **This is the security gate for an E2E *downgrade*.**
/// `disable_sync_encryption` decrypts every log and re-uploads it as
/// plaintext; without this filter the secrets carried by credential
/// events would land on the remote in the clear. The credentials stay in
/// the local keychain regardless — they are simply purged from the sync
/// storage, which is exactly the behaviour the user picked for "E2E off".
pub fn strip_credential_events(log: &LogFile) -> SyncResult<LogFile> {
    let kept: Vec<EventEnvelope> = log
        .into_envelopes()?
        .into_iter()
        .filter(|env| {
            !matches!(
                env.event,
                SyncEvent::CredentialSet(_) | SyncEvent::CredentialCleared(_)
            )
        })
        .collect();
    LogFile::from_envelopes(log.name.device_id.clone(), log.name.timestamp, &kept)
}

/// Strip the `credentials` block from a snapshot body — the snapshot
/// counterpart to [`strip_credential_events`]. Used on E2E downgrade
/// before the snapshot is re-uploaded as plaintext, so the secrets the
/// encrypted snapshot carried never reach the remote in the clear. The
/// account *metadata* (the `accounts` block) is left intact — only the
/// secret block is removed.
pub fn strip_credentials_from_snapshot(snapshot: &mut Snapshot) {
    if let Some(obj) = snapshot.body.as_object_mut() {
        obj.remove("credentials");
    }
}

/// Reduce a raw log fetched during an E2E downgrade to its plaintext form,
/// tolerating logs that a prior *interrupted* downgrade already rewrote as
/// plaintext. AES-GCM authenticates, so a plaintext (non-ciphertext) blob
/// fails to decrypt and is returned verbatim, while a genuinely encrypted
/// log is decrypted. This is what makes a retried `disable_sync_encryption`
/// idempotent instead of choking on a half-converted dataset — the strict
/// `EncryptingAdapter::fetch_new_logs` would error on the already-plaintext
/// logs. (A genuinely corrupt/tampered ciphertext that fails to decrypt is
/// returned as-is and then surfaces downstream when the strip can't parse it
/// as JSONL — it is never silently accepted as valid events.)
pub fn downgrade_log_to_plaintext(dek: &[u8; sync_core::KEY_LEN], raw: LogFile) -> LogFile {
    match sync_core::decrypt(dek, &raw.bytes) {
        Ok(decrypted) => LogFile {
            name: raw.name,
            bytes: decrypted,
        },
        Err(_) => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sync_core::{DeviceId, EventPayload, IdPayload};

    fn ts() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-06-08T09:14:22Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn strip_drops_credential_events_and_their_secrets() {
        let dev = DeviceId::from_string("dev-a".into());
        let envs = vec![
            EventEnvelope {
                id: "1".into(),
                device_id: dev.clone(),
                timestamp: ts(),
                event: SyncEvent::EventCreated(EventPayload {
                    id: "e1".into(),
                    fields: serde_json::json!({ "title": "Meeting" }),
                }),
            },
            EventEnvelope {
                id: "2".into(),
                device_id: dev.clone(),
                timestamp: ts(),
                event: SyncEvent::CredentialSet(CredentialPayload {
                    account_id: "acc".into(),
                    slot: "password".into(),
                    secret: "s3cr3t-p4ss".into(),
                }),
            },
            EventEnvelope {
                id: "3".into(),
                device_id: dev.clone(),
                timestamp: ts(),
                event: SyncEvent::AccountDeleted(IdPayload { id: "acc".into() }),
            },
            EventEnvelope {
                id: "4".into(),
                device_id: dev.clone(),
                timestamp: ts(),
                event: SyncEvent::CredentialCleared(CredentialSlotPayload {
                    account_id: "acc".into(),
                    slot: "password".into(),
                }),
            },
        ];
        let log = LogFile::from_envelopes(dev.clone(), ts(), &envs).unwrap();

        let stripped = strip_credential_events(&log).unwrap();
        let out = stripped.into_envelopes().unwrap();

        // Only the two non-credential events survive, in order.
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].event, SyncEvent::EventCreated(_)));
        assert!(matches!(out[1].event, SyncEvent::AccountDeleted(_)));

        // CRITICAL: no secret and no credential event type may remain in
        // the bytes that the downgrade would push as plaintext.
        let text = String::from_utf8(stripped.bytes.clone()).unwrap();
        assert!(
            !text.contains("s3cr3t-p4ss"),
            "secret leaked into plaintext log"
        );
        assert!(!text.contains("credential.set"));
        assert!(!text.contains("credential.cleared"));
    }

    #[test]
    fn downgrade_tolerates_plaintext_and_decrypts_ciphertext() {
        let dek = sync_core::fresh_data_key();
        let dev = DeviceId::from_string("dev-a".into());
        let env = EventEnvelope {
            id: "1".into(),
            device_id: dev.clone(),
            timestamp: ts(),
            event: SyncEvent::EventCreated(EventPayload {
                id: "e1".into(),
                fields: serde_json::json!({ "title": "x" }),
            }),
        };
        let plaintext_log = LogFile::from_envelopes(dev.clone(), ts(), &[env]).unwrap();

        // A plaintext log (as a prior interrupted downgrade would leave it)
        // passes through verbatim.
        let out = downgrade_log_to_plaintext(&dek, plaintext_log.clone());
        assert_eq!(out.bytes, plaintext_log.bytes);

        // A genuinely encrypted log (normal E2E state) decrypts back to the
        // same plaintext.
        let ciphertext = sync_core::encrypt(&dek, &plaintext_log.bytes).unwrap();
        let encrypted_log = LogFile {
            name: plaintext_log.name.clone(),
            bytes: ciphertext,
        };
        let out2 = downgrade_log_to_plaintext(&dek, encrypted_log);
        assert_eq!(out2.bytes, plaintext_log.bytes);
    }
}
