//! Snapshot builder + applier for the cross-device sync layer
//! (DESIGN.md §19.10, Phase Sg).
//!
//! Pulls the calendar / event / task / task-list / colour-label rows
//! (via [`SyncStore::dump_for_snapshot`]) together with the whitelisted
//! `user_prefs` rows, the non-secret account rows and — when E2E is on —
//! the account secrets into a single [`AperioSnapshotBody`], and wraps it
//! into a [`Snapshot`] ready for `SyncAdapter::push_snapshot`.
//!
//! Goes the other way on the apply path: parse the body, restore every
//! section through the [`SyncStore`] (which doesn't fire the event-log
//! writer, so no loop) and write the account secrets back through the
//! [`SecretStore`].
//!
//! The builder reaches storage only through [`SyncStore`] + [`SecretStore`]
//! so the same logic serves desktop (SQLite + keyring) and mobile (the app
//! sandbox + Keychain/Keystore) — the platform-specific SQL and credential
//! plumbing live in each platform's trait impl, never here.
//!
//! ## Schema
//!
//! ```jsonc
//! {
//!   "calendars":    [Calendar...],
//!   "events":       [Event...],
//!   "task_lists":   [TaskList...],
//!   "tasks":        [Task...],
//!   "color_labels": [ColorLabel...],
//!   "settings":     { "appearance.darkMode": "true", ... }
//! }
//! ```
//!
//! Top-level keys are open (no `deny_unknown_fields`) so a future
//! Aperio adding a new section doesn't sink older readers — they
//! ignore the unknown section and keep going.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use adapter_local::SnapshotDump;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sync_core::{LogFile, Snapshot, SyncError, SyncEvent, SyncResult};
use tracing::warn;

use crate::whitelist::is_synced_key;
use crate::{SecretSlot, SecretStore, SyncStore};

/// The body of an Aperio snapshot. Lives between
/// `serde_json::Value` (what `sync_core::Snapshot.body` carries) and
/// the typed dump structs that the store knows how to round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AperioSnapshotBody {
    /// Calendar/event/task/list/label dump from
    /// [`SyncStore::dump_for_snapshot`]. The adapter-local
    /// crate owns the schema for each section.
    #[serde(flatten)]
    pub dump: SnapshotDump,
    /// Whitelisted `user_prefs` rows. Key → opaque string value,
    /// same shape the storage layer holds them in.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// Account metadata (display_name, kind, non-secret config).
    /// §19.11.8 wants this in the snapshot so a fresh device can
    /// surface the "connect each account" wizard after onboarding
    /// — the user gets to see which providers had been set up on
    /// the other device(s) and which credentials they still need
    /// to re-enter on THIS one. Credentials themselves are
    /// device-local (kept in the OS keychain), so we never sync
    /// the passwords / OAuth tokens — only the config that
    /// identifies which keychain entry each account needs.
    #[serde(default)]
    pub accounts: Vec<SnapshotAccount>,
    /// Account secrets — present ONLY when E2E is enabled, in which case
    /// the whole snapshot body is an encrypted blob. This is what lets a
    /// freshly-joined device (and any device after a log compaction)
    /// recover the credentials without re-entry. When E2E is off the
    /// build path leaves this empty and `skip_serializing_if` keeps the
    /// key out of the (plaintext) body entirely. The E2E-disable path
    /// strips it before any plaintext re-upload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<SnapshotCredential>,
    /// The ids of the events still in log files that this snapshot already
    /// reflects (DESIGN-sync-snapshot-coverage.md). A device that takes the
    /// snapshot records them as applied and can then fetch every log file
    /// and apply only the rest: a session file named before the snapshot
    /// may have grown after it. `None` when the compactor could not vouch
    /// for its applied record — a snapshot as before, by timestamp only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_event_ids: Option<Vec<String>>,
}

impl AperioSnapshotBody {
    /// The named deletes in `logs` whose row this snapshot does not hold
    /// (DESIGN-sync-snapshot-coverage.md, the rebuild). A snapshot reflects
    /// a delete as an absent row, and applying it only upserts, so a device
    /// still holding the row would keep it. Taken out of the applied record,
    /// such a delete is applied again: it removes the leftover row, and where
    /// the row is already gone it does nothing. A delete whose row the
    /// snapshot does hold stays applied — the row came back after it. Empty
    /// for a snapshot that names nothing.
    pub fn deletes_to_replay(&self, logs: &[LogFile]) -> Vec<String> {
        let Some(covered) = &self.covered_event_ids else {
            return Vec::new();
        };
        let covered: HashSet<&str> = covered.iter().map(String::as_str).collect();
        fn ids<'a, T>(rows: &'a [T], id: impl Fn(&'a T) -> &'a str) -> HashSet<&'a str> {
            rows.iter().map(id).collect()
        }
        let dump = &self.dump;
        let events = ids(&dump.events, |r| r.id.as_str());
        let groups = ids(&dump.event_groups, |r| r.id.as_str());
        let tasks = ids(&dump.tasks, |r| r.id.as_str());
        let lists = ids(&dump.task_lists, |r| r.id.as_str());
        let sections = ids(&dump.sections, |r| r.id.as_str());
        let calendars = ids(&dump.calendars, |r| r.id.as_str());
        let labels = ids(&dump.color_labels, |r| r.id.as_str());
        let markers = ids(&dump.day_markers, |r| r.id.as_str());
        let accounts = ids(&self.accounts, |r| r.id.as_str());
        let mut replay = Vec::new();
        for log in logs {
            let Ok(envelopes) = log.into_envelopes() else {
                continue;
            };
            for envelope in envelopes {
                if !covered.contains(envelope.id.as_str()) {
                    continue;
                }
                let held = match &envelope.event {
                    SyncEvent::EventDeleted(p) => events.contains(p.id.as_str()),
                    SyncEvent::EventGroupDissolved(p) => groups.contains(p.id.as_str()),
                    SyncEvent::TaskDeleted(p) => tasks.contains(p.id.as_str()),
                    SyncEvent::TaskListDeleted(p) => lists.contains(p.id.as_str()),
                    SyncEvent::SectionDeleted(p) => sections.contains(p.id.as_str()),
                    SyncEvent::CalendarDeleted(p) => calendars.contains(p.id.as_str()),
                    SyncEvent::ColorLabelDeleted(p) => labels.contains(p.id.as_str()),
                    SyncEvent::DayMarkerDeleted(p) => markers.contains(p.id.as_str()),
                    SyncEvent::AccountDeleted(p) => accounts.contains(p.id.as_str()),
                    _ => continue,
                };
                if !held {
                    replay.push(envelope.id);
                }
            }
        }
        replay
    }
}

/// One account secret carried in [`AperioSnapshotBody::credentials`].
/// Only ever serialised inside an E2E-encrypted snapshot. `slot` is the
/// keychain slot wire name; only the syncable slots (`password`,
/// `refresh_token`, `api_token`) are ever produced or applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotCredential {
    pub account_id: String,
    pub slot: String,
    pub secret: String,
}

/// Non-secret account row carried in [`AperioSnapshotBody::accounts`].
/// Mirrors the columns of `accounts` we want to round-trip across
/// devices; deliberately excludes the keychain-backed secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotAccount {
    pub id: String,
    pub adapter_kind: String,
    pub display_name: String,
    /// JSON string of the adapter's non-secret config (server
    /// URLs, client_ids, etc.). Stored opaquely; the snapshot
    /// applier doesn't validate the shape.
    pub config_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Result of applying a snapshot body. Merges
/// [`adapter_local::SnapshotApplyReport`] from adapter-local plus
/// settings/account counters.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct SnapshotApplyOutcome {
    pub rows_applied: usize,
    pub rows_failed: usize,
    pub settings_applied: usize,
    pub settings_failed: usize,
    pub accounts_applied: usize,
    pub accounts_failed: usize,
    /// How many events the snapshot named as held and were recorded as
    /// applied — `None` for a snapshot that names nothing
    /// (DESIGN-sync-snapshot-coverage.md).
    pub covered_events: Option<usize>,
}

impl SnapshotApplyOutcome {
    fn merge_rows(&mut self, r: adapter_local::SnapshotApplyReport) {
        self.rows_applied += r.applied;
        self.rows_failed += r.failed;
    }
}

/// Helper that knows how to build snapshots and how to apply them.
///
/// Kept independent of the orchestrator so the same instance can be
/// driven from the compactor, the onboarding flow, and (later) any
/// "export local state to JSON" debug command.
pub struct SnapshotBuilder {
    /// Local store seam — the calendar/event/task rows plus the
    /// whitelisted settings and the non-secret account rows.
    store: Arc<dyn SyncStore>,
    /// Credential store seam — the per-account secrets carried only
    /// inside an E2E-encrypted body.
    secrets: Arc<dyn SecretStore>,
    app_version: String,
}

impl SnapshotBuilder {
    pub fn new(
        store: Arc<dyn SyncStore>,
        secrets: Arc<dyn SecretStore>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            store,
            secrets,
            app_version: app_version.into(),
        }
    }

    /// Build a [`Snapshot`] reflecting the current local state +
    /// whitelisted user_prefs, stamped `Utc::now()`. The caller is
    /// responsible for atomically updating `meta.json.snapshot_timestamp`
    /// to match after a successful `push_snapshot`.
    pub fn build(&self) -> SyncResult<Snapshot> {
        self.build_at(Utc::now(), None)
    }

    /// Like [`build`](Self::build) but stamps the snapshot with a
    /// caller-supplied `snapshot_at`. The compactor uses this to make
    /// the snapshot sort *just before* the writer's freshly-rotated
    /// session file: the snapshot then covers every event written to
    /// the now-closed pre-rotation file, while post-rotation events
    /// land in a file whose timestamp is strictly newer than the
    /// snapshot — so a device that consumes the snapshot (and advances
    /// its cursor to `snapshot_at`) still fetches them instead of
    /// skipping them as "older than the snapshot". `covered_event_ids` is
    /// what the compactor can vouch the snapshot holds
    /// ([`AperioSnapshotBody::covered_event_ids`]).
    pub fn build_at(
        &self,
        snapshot_at: DateTime<Utc>,
        covered_event_ids: Option<Vec<String>>,
    ) -> SyncResult<Snapshot> {
        let dump = self
            .store
            .dump_for_snapshot()
            .map_err(|err| SyncError::internal(format!("dump rows: {err}")))?;
        let settings = self
            .store
            .dump_synced_settings()
            .map_err(|err| SyncError::internal(format!("dump settings: {err}")))?;
        let accounts = self
            .store
            .dump_accounts()
            .map_err(|err| SyncError::internal(format!("dump accounts: {err}")))?;
        let credentials = self.dump_credentials(&accounts)?;
        let body = AperioSnapshotBody {
            dump,
            settings,
            accounts,
            credentials,
            covered_event_ids,
        };
        let body_value = serde_json::to_value(&body)?;
        Ok(Snapshot::new(
            snapshot_at,
            self.app_version.clone(),
            body_value,
        ))
    }

    /// Apply a snapshot to the local store. Parses the body, restores
    /// every section, returns a counter the caller can surface to the
    /// user.
    ///
    /// Settings are written via [`SyncStore::set_setting`] directly so
    /// they don't loop back through the event-log writer (which would
    /// emit `settings.updated` events for each restore — not what we
    /// want during onboarding).
    pub fn apply(&self, snapshot: &Snapshot) -> SyncResult<SnapshotApplyOutcome> {
        let body: AperioSnapshotBody = serde_json::from_value(snapshot.body.clone())?;
        let mut outcome = SnapshotApplyOutcome::default();
        let report = self
            .store
            .apply_snapshot_dump(&body.dump)
            .map_err(|err| SyncError::internal(format!("apply rows: {err}")))?;
        outcome.merge_rows(report);

        for (key, value) in &body.settings {
            // Hard-gate against the whitelist on apply too — a
            // malicious or buggy peer can't smuggle a non-synced
            // key (e.g. someone's keychain reference) into the
            // settings table.
            if !is_synced_key(key) {
                warn!(
                    key = %key,
                    "snapshot settings key not on whitelist; dropping",
                );
                outcome.settings_failed += 1;
                continue;
            }
            match self.store.set_pref(key, value) {
                Ok(()) => outcome.settings_applied += 1,
                Err(err) => {
                    warn!(
                        key = %key,
                        ?err,
                        "failed to apply snapshot setting",
                    );
                    outcome.settings_failed += 1;
                }
            }
        }

        // §19.11.8: restore the non-secret account rows. The
        // store does a flat upsert keyed by `id` — the LOCAL
        // account (id = "local") is always present from the
        // schema's bootstrap row, so the snapshot's local entry
        // (if any) just overwrites it harmlessly.
        for acc in &body.accounts {
            match self.store.upsert_account(acc) {
                Ok(()) => outcome.accounts_applied += 1,
                Err(err) => {
                    warn!(
                        account_id = %acc.id,
                        ?err,
                        "failed to apply snapshot account",
                    );
                    outcome.accounts_failed += 1;
                }
            }
        }

        // Account secrets — only present in E2E snapshots (the body was an
        // encrypted blob, so reaching here means it decrypted). Defense in
        // depth: only write them when THIS device's E2E is on too. A
        // credential-bearing body must never be applied on a plaintext-mode
        // device — the downgrade strip already keeps that from happening,
        // this guards against the strip ever regressing. Each slot is also
        // re-validated against the syncable allowlist so a tampered snapshot
        // can't write an access token or the E2E key. Best-effort per row.
        if self.store.e2e_enabled() {
            for cred in &body.credentials {
                if cred.account_id == "local" {
                    continue;
                }
                let Some(slot) = SecretSlot::syncable_from_wire(&cred.slot) else {
                    warn!(slot = %cred.slot, "snapshot credential: non-syncable slot dropped");
                    continue;
                };
                if let Err(err) = self.secrets.store(&cred.account_id, slot, &cred.secret) {
                    warn!(
                        account_id = %cred.account_id,
                        ?err,
                        "failed to apply snapshot credential",
                    );
                }
            }
        } else if !body.credentials.is_empty() {
            warn!(
                count = body.credentials.len(),
                "snapshot carried credentials but E2E is off locally; ignoring them",
            );
        }

        // The events this snapshot holds that are still in log files
        // (DESIGN-sync-snapshot-coverage.md), recorded as applied: their
        // effect is in the rows above now, so a device taking the snapshot
        // can fetch every log file and apply only the rest. A failure fails
        // the apply — without the record, that fetch would re-apply what the
        // rows hold and report false conflicts.
        if let Some(ids) = &body.covered_event_ids {
            self.store
                .mark_events_applied(ids)
                .map_err(|err| SyncError::internal(format!("record covered events: {err}")))?;
            outcome.covered_events = Some(ids.len());
        }
        Ok(outcome)
    }

    /// Account secrets for the snapshot — but ONLY when E2E is enabled,
    /// so they're written exclusively into an encrypted blob. Mirrors the
    /// live-event gate: the E2E check plus the syncable-slot allowlist
    /// (via the [`SecretSlot`] choice). When E2E is off this returns an
    /// empty vec and the snapshot carries no secrets at all. Takes the
    /// already-dumped `accounts` so the `local` account (which has no
    /// secrets anyway) is already excluded.
    fn dump_credentials(
        &self,
        accounts: &[SnapshotAccount],
    ) -> SyncResult<Vec<SnapshotCredential>> {
        if !self.store.e2e_enabled() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for acc in accounts {
            for slot in [
                SecretSlot::Password,
                SecretSlot::RefreshToken,
                SecretSlot::ApiToken,
                // Bring-your-own OAuth client secret — see `syncable_from_wire`:
                // it exists on one device only, and the refresh token beside it
                // is unusable without it.
                SecretSlot::OauthClientSecret,
            ] {
                if let Ok(secret) = self.secrets.retrieve(&acc.id, slot) {
                    out.push(SnapshotCredential {
                        account_id: acc.id.clone(),
                        slot: slot.wire_name().to_string(),
                        secret,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeSecrets, FakeStore};
    use std::sync::Mutex;

    fn builder(store: Arc<FakeStore>, secrets: Arc<FakeSecrets>) -> SnapshotBuilder {
        SnapshotBuilder::new(store, secrets, "1.0.0-test")
    }

    #[test]
    fn apply_drops_settings_not_on_whitelist() {
        // Forward-compat / hostile-peer guard: a snapshot body
        // claiming to write a non-synced key must be rejected.
        let store = Arc::new(FakeStore::default());
        let secrets = Arc::new(FakeSecrets::default());
        let mut body = AperioSnapshotBody::default();
        body.settings
            .insert("sync.deviceId".into(), "stolen-id".into());
        body.settings
            .insert("appearance.darkMode".into(), "true".into());
        let snap = Snapshot::new(
            Utc::now(),
            "1.0.0-test",
            serde_json::to_value(&body).unwrap(),
        );
        let outcome = builder(Arc::clone(&store), secrets).apply(&snap).unwrap();
        assert_eq!(outcome.settings_applied, 1);
        assert_eq!(outcome.settings_failed, 1);
        // Confirm the protected key didn't land and the legit one did.
        let prefs = store.prefs.lock().unwrap();
        assert!(!prefs.contains_key("sync.deviceId"));
        assert_eq!(
            prefs.get("appearance.darkMode").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn apply_records_the_events_the_snapshot_names_as_applied() {
        // DESIGN-sync-snapshot-coverage.md: the named events' effect is in
        // the applied rows, so they are recorded as applied with them — a
        // device taking the snapshot then fetches every file and applies
        // only the rest. A snapshot that names nothing records nothing.
        let store = Arc::new(FakeStore::default());
        let secrets = Arc::new(FakeSecrets::default());
        let body = AperioSnapshotBody {
            covered_event_ids: Some(vec!["ev-1".into(), "ev-2".into()]),
            ..Default::default()
        };
        let snap = Snapshot::new(
            Utc::now(),
            "1.0.0-test",
            serde_json::to_value(&body).unwrap(),
        );
        let outcome = builder(Arc::clone(&store), Arc::clone(&secrets))
            .apply(&snap)
            .unwrap();
        assert_eq!(outcome.covered_events, Some(2));
        let applied = store.applied.lock().unwrap().clone();
        assert!(applied.contains("ev-1") && applied.contains("ev-2"));

        let plain = Arc::new(FakeStore::default());
        let snap = Snapshot::new(
            Utc::now(),
            "1.0.0-test",
            serde_json::to_value(AperioSnapshotBody::default()).unwrap(),
        );
        let outcome = builder(Arc::clone(&plain), secrets).apply(&snap).unwrap();
        assert_eq!(outcome.covered_events, None);
        assert!(plain.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn replays_a_named_delete_only_where_the_snapshot_no_longer_holds_the_row() {
        // DESIGN-sync-snapshot-coverage.md, the rebuild: a snapshot reflects
        // a delete as an absent row and applying it only upserts, so a named
        // delete of a row the snapshot lacks must be applied again; one whose
        // row it holds (it came back) stays applied; an unnamed delete is
        // applied anyway, and a snapshot naming nothing replays nothing.
        let device = sync_core::DeviceId::from_string("dev-other".into());
        let delete = |id: &str, target: &str| sync_core::EventEnvelope {
            id: id.into(),
            device_id: device.clone(),
            timestamp: Utc::now(),
            event: SyncEvent::ColorLabelDeleted(sync_core::IdPayload { id: target.into() }),
        };
        let lines = [
            delete("ev-gone", "lbl-gone"),
            delete("ev-back", "lbl-back"),
            delete("ev-unnamed", "lbl-other"),
        ]
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
        let log = LogFile {
            name: sync_core::LogFileName::new(Utc::now(), device),
            bytes: lines.into_bytes(),
        };
        let mut body = AperioSnapshotBody {
            covered_event_ids: Some(vec!["ev-gone".into(), "ev-back".into()]),
            ..Default::default()
        };
        body.dump.color_labels.push(cal_core::ColorLabel {
            id: cal_core::ColorLabelId::new("lbl-back"),
            name: "Wieder da".into(),
            hex: "#e53935".into(),
            ad_hoc: false,
        });
        assert_eq!(
            body.deletes_to_replay(std::slice::from_ref(&log)),
            vec!["ev-gone"]
        );

        body.covered_event_ids = None;
        assert!(body.deletes_to_replay(&[log]).is_empty());
    }

    #[test]
    fn credentials_only_built_and_applied_when_e2e_on() {
        // Build side: with E2E off, no secret is read into the body even
        // when the keychain holds one.
        let store_off = Arc::new(FakeStore {
            accounts: Mutex::new(vec![SnapshotAccount {
                id: "acc-1".into(),
                adapter_kind: "caldav".into(),
                display_name: "Fastmail".into(),
                config_json: "{}".into(),
                created_at: "2026-05-24T10:00:00Z".into(),
                updated_at: "2026-05-24T10:00:00Z".into(),
            }]),
            e2e: false,
            ..Default::default()
        });
        let secrets = Arc::new(FakeSecrets::default());
        secrets
            .store("acc-1", SecretSlot::Password, "hunter2")
            .unwrap();
        let snap = builder(Arc::clone(&store_off), Arc::clone(&secrets))
            .build()
            .unwrap();
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        assert!(
            body.credentials.is_empty(),
            "E2E off must not export secrets"
        );

        // Build side with E2E on: the password is exported.
        let store_on = Arc::new(FakeStore {
            accounts: Mutex::new(vec![SnapshotAccount {
                id: "acc-1".into(),
                adapter_kind: "caldav".into(),
                display_name: "Fastmail".into(),
                config_json: "{}".into(),
                created_at: "2026-05-24T10:00:00Z".into(),
                updated_at: "2026-05-24T10:00:00Z".into(),
            }]),
            e2e: true,
            ..Default::default()
        });
        let snap_on = builder(Arc::clone(&store_on), Arc::clone(&secrets))
            .build()
            .unwrap();
        let body_on: AperioSnapshotBody = serde_json::from_value(snap_on.body.clone()).unwrap();
        assert_eq!(body_on.credentials.len(), 1);
        assert_eq!(body_on.credentials[0].account_id, "acc-1");
        assert_eq!(body_on.credentials[0].slot, "password");

        // Apply side: a credential-bearing body is ignored when local
        // E2E is off, and written through when it's on.
        let fresh_off = Arc::new(FakeStore {
            e2e: false,
            ..Default::default()
        });
        let fresh_secrets_off = Arc::new(FakeSecrets::default());
        builder(Arc::clone(&fresh_off), Arc::clone(&fresh_secrets_off))
            .apply(&snap_on)
            .unwrap();
        assert!(
            fresh_secrets_off
                .retrieve("acc-1", SecretSlot::Password)
                .is_err(),
            "E2E off locally must not import secrets",
        );

        let fresh_on = Arc::new(FakeStore {
            e2e: true,
            ..Default::default()
        });
        let fresh_secrets_on = Arc::new(FakeSecrets::default());
        builder(Arc::clone(&fresh_on), Arc::clone(&fresh_secrets_on))
            .apply(&snap_on)
            .unwrap();
        assert_eq!(
            fresh_secrets_on
                .retrieve("acc-1", SecretSlot::Password)
                .unwrap(),
            "hunter2",
        );
    }
}
