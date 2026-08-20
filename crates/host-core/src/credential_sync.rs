//! The single chokepoint for cross-device credential sync.
//!
//! Account secrets (CalDAV / WebDAV passwords, OAuth refresh tokens, API
//! tokens) only ever leave the device through the sync log when
//! **end-to-end encryption is enabled** — so a secret always lives inside
//! an encrypted log blob, never plaintext on the remote. This module is
//! the *only* place that turns a stored secret into a `credential.*`
//! event, so the E2E gate lives in exactly one auditable spot.
//!
//! The gate is stacked on purpose:
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
//!   3. **The account must travel.** A credential is keyed to an account id,
//!      and an account that belongs to the device that made it never puts that
//!      id on the wire — so its secret would arrive keyed to a row the other
//!      device does not have, for an adapter it does not run. The kind's own
//!      declared capabilities answer; see
//!      [`crate::accounts::travels_between_devices`].
//!
//! The matching purge side — making sure these events never survive an
//! E2E *downgrade* — lives in the `disable_sync_encryption` flow.

use sync_core::{
    CredentialPayload, CredentialSlotPayload, EventEnvelope, LogFile, Snapshot, SyncEvent,
    SyncResult,
};
use sync_engine::{EventLogWriter, SecretSlot, SecretStore};

use crate::accounts::{AccountsRepo, AdapterKind};
use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// User-prefs key mirroring `meta.json.e2e_enabled` — the local flag that
/// decides whether the sync adapter wraps in `EncryptingAdapter` and whether
/// credentials may enter the sync log. Owned here (the single auditable home
/// for the credential-sync gate); `commands::sync` re-exports it.
pub use sync_engine::whitelist::PREF_E2E_ENABLED;

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

/// The kind stored for this account id, or `None` when this device holds no
/// row under it.
///
/// The two emitters below take an account id and need a kind; they already hold
/// the connection the row lives in, so they read it rather than making nine call
/// sites each derive an answer that only has to be wrong once. A read that fails
/// is `None` for the same reason a missing row is: nothing on this device can
/// say what kind that id names. The callers decide what to do about it — and
/// they decide differently, see each one.
fn stored_adapter_kind(conn: &SharedConn, account_id: &str) -> Option<AdapterKind> {
    match AccountsRepo::new(conn).get(account_id) {
        Ok(account) => account.map(|account| account.adapter_kind),
        Err(err) => {
            tracing::warn!(?err, account_id, "credential emit: account lookup failed");
            None
        }
    }
}

/// Emit a `credential.set` event for one account secret — but ONLY when
/// E2E is on, the slot is on the syncable allowlist *and* the account is one
/// that travels between devices. Otherwise it is a no-op and the secret stays
/// device-local (keychain only). Call this right after a successful
/// secret-store write.
pub fn emit_credential_set(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    manager: &plugin_core::PluginManager,
    account_id: &str,
    slot: SecretSlot,
    secret: &str,
) {
    // Defense in depth: refuse non-syncable slots (access_token / the E2E
    // key) before the secret can ever reach an event.
    if SecretSlot::syncable_from_wire(slot.wire_name()).is_none() {
        return;
    }
    if !e2e_enabled(conn) {
        return;
    }
    // The hand-written `local` skip that used to stand here is subsumed by the
    // predicate: `local` is host-internal, as is every other kind that belongs
    // to the device that made it.
    let Some(kind) = stored_adapter_kind(conn, account_id) else {
        // A secret is the one thing that must never go out on a guess, so an id
        // this device cannot resolve stays home. Every caller writes the secret
        // against a row it has just created or just read, which makes this a
        // can't-happen — logged rather than silent, because reaching it means a
        // caller emitted for an account that was already gone.
        tracing::warn!(account_id, "credential.set: no account row; not emitting");
        return;
    };
    if !crate::accounts::travels_between_devices(manager, kind.as_str()) {
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
    manager: &plugin_core::PluginManager,
    account_id: &str,
    slot: SecretSlot,
) {
    if SecretSlot::syncable_from_wire(slot.wire_name()).is_none() {
        return;
    }
    if !e2e_enabled(conn) {
        return;
    }
    match stored_adapter_kind(conn, account_id) {
        // The ordinary case: the row is here and its kind answers.
        Some(kind) if !crate::accounts::travels_between_devices(manager, kind.as_str()) => return,
        Some(_) => {}
        // No row — which a caller clearing a slot as part of tearing an account
        // down would see, since the row may already be gone. This one goes the
        // OTHER way from `emit_credential_set` above, and deliberately: the
        // event carries no secret, so sending one that turns out to be
        // unnecessary costs an id and a slot name inside an already-encrypted
        // log, while dropping it leaves a revoked credential alive in another
        // device's keychain with nothing left to say so.
        None => tracing::debug!(
            account_id,
            "credential.cleared: no account row; emitting anyway (a clear must not be lost)",
        ),
    }
    event_log.append(SyncEvent::CredentialCleared(CredentialSlotPayload {
        account_id: account_id.to_string(),
        slot: slot.wire_name().to_string(),
    }));
}
/// `user_prefs` key recording which credential-slot generation this device has
/// already pushed into the log. Bumping [`SLOT_BACKFILL_VERSION`] makes every
/// device re-run [`emit_all_local_credentials`] exactly once.
const SLOT_BACKFILL_PREF: &str = "credentialSync.slotBackfillVersion";

/// The current generation. 1 = the OAuth client secret joined the syncable
/// slots (2026-08), and refresh tokens stored before the E2E-enable bulk emit
/// existed had never been pushed either.
const SLOT_BACKFILL_VERSION: i64 = 1;

/// One-time re-emit of every local credential, per slot generation.
///
/// Two real datasets needed it on the day it was written. An account whose
/// refresh token was stored before the E2E-enable flow learned to bulk-emit
/// never had that token on the wire at all. And every bring-your-own OAuth
/// client secret predates its own syncability by definition. Both leave a
/// second device with an account row it can never open — the sidebar says
/// "no calendars found" and nothing explains why.
///
/// Runs only with E2E on (the same gate every emit obeys), and does NOT mark
/// itself done while E2E is off — a dataset that turns E2E on next month must
/// still get its backfill. Appending a duplicate `credential.set` for a secret
/// the wire already carried is harmless: the applier overwrites the same
/// keychain entry with the same value.
pub fn backfill_new_syncable_slots(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    manager: &plugin_core::PluginManager,
    secrets: &dyn SecretStore,
) {
    if !e2e_enabled(conn) {
        return;
    }
    let prefs = UserPrefsRepo::new(conn);
    let done = prefs
        .get(SLOT_BACKFILL_PREF)
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(0);
    if done >= SLOT_BACKFILL_VERSION {
        return;
    }
    emit_all_local_credentials(event_log, conn, manager, secrets);
    if let Err(err) = prefs.set(SLOT_BACKFILL_PREF, &SLOT_BACKFILL_VERSION.to_string()) {
        // Failing to record it means one redundant re-emit next launch —
        // annoying in the log, harmless on the wire.
        tracing::warn!(?err, "credential backfill: couldn't record the version");
    }
}

/// Push every local account secret into the (now-encrypted) sync log —
/// used when E2E is turned on *after* accounts already exist, so the
/// user's other devices pick them up without re-entering. Each secret
/// routes through [`emit_credential_set`], so this is a no-op unless E2E
/// is actually on and only the syncable slots are ever touched. Best
/// effort: a missing slot is normal (not every account has every slot)
/// and a keychain read error for one slot doesn't abort the rest.
///
/// An account that stays on this device is skipped whole. Its row never
/// reaches the other devices — see [`crate::accounts::travels_between_devices`]
/// — so a secret for it would arrive keyed to an account id nothing there
/// knows, and be stored for an adapter that device does not use. That is the
/// argument [`SecretSlot::KeyPassphrase`] and [`SecretSlot::OauthClientSecret`]
/// already make slot by slot: a credential the receiver cannot use is exposure
/// bought for nothing. The `PluginManager` is here because the answer is the
/// adapter's own — read off its declared capabilities, never off a list of
/// names kept in the host.
pub fn emit_all_local_credentials(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    manager: &plugin_core::PluginManager,
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
        // [`emit_credential_set`] asks the same question per secret; skipping
        // the whole account here saves the keychain reads for one that stays.
        if !crate::accounts::travels_between_devices(manager, account.adapter_kind.as_str()) {
            continue;
        }
        for slot in [
            SecretSlot::Password,
            SecretSlot::RefreshToken,
            SecretSlot::ApiToken,
            // The bring-your-own OAuth client secret. Only user-pasted values
            // ever occupy this slot (the built-in posture persists nothing),
            // and the refresh token above is unusable without it.
            SecretSlot::OauthClientSecret,
        ] {
            // Read through the injected platform secret store (the desktop
            // keyring; the mobile keychain bridge) — never a hard-coded backend.
            if let Ok(secret) = secrets.retrieve(&account.id, slot) {
                emit_credential_set(event_log, conn, manager, &account.id, slot, &secret);
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
    use crate::db::DbHandle;
    use std::sync::Arc;
    use sync_core::{DeviceId, EventPayload, IdPayload};
    use tempfile::TempDir;

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

    // ── whose credentials may leave this device ───────────────────────────

    /// The secret every emit test offers. Distinctive enough that finding it
    /// anywhere in the log bytes means it really was written.
    const SECRET: &str = "s3cr3t-p4ss";

    /// Appended last by [`flush_and_read`] — see there.
    const SENTINEL: &str = "sentinel-after-the-emits";

    /// A manager holding exactly one real DATA adapter, so the "it travels"
    /// case is answered by a shipped `plugin.json` and not by this test. iCal
    /// is the cheapest one: no keychain secret, no network on register. The
    /// predicate's own tests use the same fixture, next to the predicate.
    fn manager_with_ical() -> plugin_core::PluginManager {
        let manager = plugin_core::PluginManager::new("0.1.0");
        let manifest = plugin_core::manifest::PluginManifest::from_bytes(include_bytes!(
            "../../adapter-ical-plugin/plugin.json"
        ))
        .expect("the shipped iCal manifest parses");
        let descriptor = unsafe { adapter_ical_plugin::build_descriptor() };
        manager
            .register_static(manifest, descriptor, adapter_ical_plugin::DESTROY_FN)
            .expect("register the static iCal plugin");
        manager
    }

    /// A device with E2E on, i.e. gate 1 open — so what the tests below observe
    /// is the account gate and nothing else.
    fn e2e_device() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        UserPrefsRepo::new(&db.shared())
            .set(PREF_E2E_ENABLED, "true")
            .expect("turn E2E on");
        (dir, db)
    }

    /// Everything the writer actually put on disk, once it is certainly done.
    ///
    /// The drain task is asynchronous, so "nothing was emitted" cannot be
    /// asserted by sleeping and hoping. This appends a sentinel event LAST and
    /// waits for THAT line: the queue is ordered, so once the sentinel is on
    /// disk anything the emitters appended before it is too — and whatever is
    /// missing was never appended at all.
    async fn flush_and_read(tmp: &TempDir, writer: Arc<EventLogWriter>) -> String {
        writer.append(SyncEvent::EventDeleted(IdPayload {
            id: SENTINEL.to_string(),
        }));
        drop(writer);
        let pending = tmp.path().join("sync").join("log").join("pending");
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let Ok(mut entries) = tokio::fs::read_dir(&pending).await else {
                continue;
            };
            let Ok(Some(entry)) = entries.next_entry().await else {
                continue;
            };
            let Ok(bytes) = tokio::fs::read(entry.path()).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.contains(SENTINEL) {
                return text;
            }
        }
        panic!("the event-log writer never flushed the sentinel within 2 s");
    }

    #[tokio::test]
    async fn a_credential_for_an_account_that_travels_is_emitted() {
        // The ordinary case: connect a feed on the laptop with E2E on, and the
        // phone can use it without the password being typed again.
        let (tmp, db) = e2e_device();
        let shared = db.shared();
        let manager = manager_with_ical();
        let account = AccountsRepo::new(&shared)
            .create(AdapterKind::new("ical"), "Team feed", "{}")
            .unwrap();
        let writer = EventLogWriter::spawn(
            tmp.path().to_path_buf(),
            DeviceId::from_string("dev-travels".into()),
        );

        emit_credential_set(
            &writer,
            &shared,
            &manager,
            &account.id,
            SecretSlot::Password,
            SECRET,
        );
        emit_credential_cleared(
            &writer,
            &shared,
            &manager,
            &account.id,
            SecretSlot::Password,
        );

        let text = flush_and_read(&tmp, writer).await;
        assert!(text.contains("credential.set"), "got: {text}");
        assert!(text.contains("credential.cleared"), "got: {text}");
        assert!(text.contains(&account.id));
        assert!(
            text.contains(SECRET),
            "carrying the secret is the whole point of the event",
        );
    }

    #[tokio::test]
    async fn a_credential_for_a_host_internal_account_is_not_emitted() {
        // The device's own calendar is backed by an OS permission grant on THIS
        // phone. Its row never crosses the wire, so a secret keyed to its id
        // would land on a device that has no such row and no such adapter —
        // exposure bought for nothing.
        let (tmp, db) = e2e_device();
        let shared = db.shared();
        let manager = manager_with_ical();
        let account = AccountsRepo::new(&shared)
            .create(
                AdapterKind::new(AdapterKind::DEVICE_CALENDAR),
                "This phone",
                "{}",
            )
            .unwrap();
        let writer = EventLogWriter::spawn(
            tmp.path().to_path_buf(),
            DeviceId::from_string("dev-stays".into()),
        );

        emit_credential_set(
            &writer,
            &shared,
            &manager,
            &account.id,
            SecretSlot::Password,
            SECRET,
        );
        emit_credential_cleared(
            &writer,
            &shared,
            &manager,
            &account.id,
            SecretSlot::Password,
        );

        let text = flush_and_read(&tmp, writer).await;
        assert!(!text.contains("credential.set"), "got: {text}");
        assert!(!text.contains("credential.cleared"), "got: {text}");
        assert!(!text.contains(&account.id));
        assert!(
            !text.contains(SECRET),
            "a secret for an account that stays on this device reached the log",
        );
    }

    #[tokio::test]
    async fn a_clear_still_travels_when_the_account_row_is_already_gone() {
        // The two emitters answer the missing row in opposite directions, which
        // is the decision this test pins down. A `set` carries a secret and must
        // never send one on a guess about a kind nothing here can name. A
        // `cleared` carries none: dropping it would leave a revoked credential
        // alive in another device's keychain with nothing left to say so, while
        // sending one that turns out to be unnecessary is a delete of a slot the
        // receiver does not have.
        let (tmp, db) = e2e_device();
        let shared = db.shared();
        let manager = manager_with_ical();
        let writer = EventLogWriter::spawn(
            tmp.path().to_path_buf(),
            DeviceId::from_string("dev-gone".into()),
        );

        emit_credential_set(
            &writer,
            &shared,
            &manager,
            "already-deleted",
            SecretSlot::Password,
            SECRET,
        );
        emit_credential_cleared(
            &writer,
            &shared,
            &manager,
            "already-deleted",
            SecretSlot::Password,
        );

        let text = flush_and_read(&tmp, writer).await;
        assert!(
            text.contains("credential.cleared"),
            "a revocation must not be lost because the row it names is gone: {text}",
        );
        assert!(!text.contains("credential.set"), "got: {text}");
        assert!(
            !text.contains(SECRET),
            "a secret went out for an id this device cannot resolve",
        );
    }
}
