//! Snapshot builder + applier for the cross-device sync layer
//! (DESIGN.md §19.10, Phase Sg).
//!
//! Pulls the `LocalAdapter::dump_for_snapshot` (calendars / events /
//! tasks / task lists / color labels) together with the whitelisted
//! `user_prefs` rows into a single [`AperioSnapshotBody`] and wraps
//! it into a [`Snapshot`] ready for `SyncAdapter::push_snapshot`.
//!
//! Goes the other way on the apply path: parse the body, restore
//! every section via the corresponding `upsert_*_from_sync`
//! helpers, and write the settings back via
//! [`UserPrefsRepo::set`] (which doesn't fire the event-log
//! writer, so no loop).
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

use std::collections::BTreeMap;
use std::sync::Arc;

use cal_adapter_local::{LocalAdapter, SnapshotApplyReport, SnapshotDump};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sync_core::{Snapshot, SyncError, SyncResult};
use tracing::warn;

use crate::db::SharedConn;
use crate::event_log::whitelist::{is_synced_key, SYNC_WHITELIST};
use crate::user_prefs::UserPrefsRepo;

/// The body of an Aperio snapshot. Lives between
/// `serde_json::Value` (what `sync_core::Snapshot.body` carries) and
/// the typed dump structs that the adapter knows how to round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AperioSnapshotBody {
    /// Calendar/event/task/list/label dump from
    /// [`LocalAdapter::dump_for_snapshot`]. The cal-adapter-local
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
}

/// Non-secret account row carried in [`AperioSnapshotBody.accounts`].
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
/// [`SnapshotApplyReport`] from cal-adapter-local plus a settings
/// counter.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct SnapshotApplyOutcome {
    pub rows_applied: usize,
    pub rows_failed: usize,
    pub settings_applied: usize,
    pub settings_failed: usize,
    pub accounts_applied: usize,
    pub accounts_failed: usize,
}

impl SnapshotApplyOutcome {
    fn merge_rows(&mut self, r: SnapshotApplyReport) {
        self.rows_applied += r.applied;
        self.rows_failed += r.failed;
    }
}

/// Helper that knows how to build snapshots and how to apply them.
///
/// Kept independent of [`SyncOrchestrator`] so the same instance can
/// be driven from the compactor, the onboarding flow, and (later)
/// any "export local state to JSON" debug command.
pub struct SnapshotBuilder {
    db: SharedConn,
    /// Apply path uses the adapter for `upsert_*_from_sync` calls.
    /// Build path uses it for `dump_for_snapshot`. Same Arc clone
    /// works for both.
    adapter: Arc<LocalAdapter>,
    app_version: String,
}

impl SnapshotBuilder {
    pub fn new(db: SharedConn, adapter: Arc<LocalAdapter>, app_version: impl Into<String>) -> Self {
        Self {
            db,
            adapter,
            app_version: app_version.into(),
        }
    }

    /// Build a [`Snapshot`] reflecting the current local SQLite
    /// state + whitelisted user_prefs, stamped `Utc::now()`. The
    /// caller is responsible for atomically updating
    /// `meta.json.snapshot_timestamp` to match after a successful
    /// `push_snapshot`.
    pub fn build(&self) -> SyncResult<Snapshot> {
        self.build_at(Utc::now())
    }

    /// Like [`build`](Self::build) but stamps the snapshot with a
    /// caller-supplied `snapshot_at`. The compactor uses this to make
    /// the snapshot sort *just before* the writer's freshly-rotated
    /// session file: the snapshot then covers every event written to
    /// the now-closed pre-rotation file, while post-rotation events
    /// land in a file whose timestamp is strictly newer than the
    /// snapshot — so a device that consumes the snapshot (and advances
    /// its cursor to `snapshot_at`) still fetches them instead of
    /// skipping them as "older than the snapshot".
    pub fn build_at(&self, snapshot_at: DateTime<Utc>) -> SyncResult<Snapshot> {
        let dump = self
            .adapter
            .dump_for_snapshot()
            .map_err(|err| SyncError::internal(format!("dump rows: {err}")))?;
        let settings = self.dump_settings()?;
        let accounts = self.dump_accounts()?;
        let body = AperioSnapshotBody {
            dump,
            settings,
            accounts,
        };
        let body_value = serde_json::to_value(&body)?;
        Ok(Snapshot::new(
            snapshot_at,
            self.app_version.clone(),
            body_value,
        ))
    }

    /// Apply a snapshot to the local SQLite. Parses the body,
    /// restores every section, returns a counter the caller can
    /// surface to the user.
    ///
    /// Settings are written via [`UserPrefsRepo::set`] directly so
    /// they don't loop back through the event-log writer (which
    /// would emit `settings.updated` events for each restore — not
    /// what we want during onboarding).
    pub fn apply(&self, snapshot: &Snapshot) -> SyncResult<SnapshotApplyOutcome> {
        let body: AperioSnapshotBody = serde_json::from_value(snapshot.body.clone())?;
        let mut outcome = SnapshotApplyOutcome::default();
        let report = self
            .adapter
            .apply_snapshot_dump(&body.dump)
            .map_err(|err| SyncError::internal(format!("apply rows: {err}")))?;
        outcome.merge_rows(report);

        let prefs = UserPrefsRepo::new(&self.db);
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
            match prefs.set(key, value) {
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
        // applier does a flat upsert keyed by `id` — the LOCAL
        // account (id = "local") is always present from the
        // schema's bootstrap row, so the snapshot's local entry
        // (if any) just overwrites it harmlessly.
        for acc in &body.accounts {
            match upsert_snapshot_account(&self.db, acc) {
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
        Ok(outcome)
    }

    /// Read every account row except the implicit `local` one into
    /// a `Vec<SnapshotAccount>` for the §19.11.8 wizard. The
    /// `local` account is skipped because the schema's bootstrap
    /// row recreates it on every device; including it would just
    /// bloat the snapshot.
    ///
    /// Secrets are NOT touched here — only the non-secret columns
    /// (`id`, `adapter_kind`, `display_name`, `config_json`,
    /// timestamps) ride along. Credentials stay device-local in
    /// the OS keychain.
    fn dump_accounts(&self) -> SyncResult<Vec<SnapshotAccount>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, adapter_kind, display_name, config_json,
                        created_at, updated_at
                   FROM accounts
                  WHERE id != 'local'
                  ORDER BY display_name COLLATE NOCASE",
            )
            .map_err(|err| SyncError::internal(format!("dump accounts prepare: {err}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SnapshotAccount {
                    id: row.get(0)?,
                    adapter_kind: row.get(1)?,
                    display_name: row.get(2)?,
                    config_json: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|err| SyncError::internal(format!("dump accounts query: {err}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|err| SyncError::internal(format!("dump accounts row: {err}")))?);
        }
        Ok(out)
    }

    /// Read the current values of every whitelisted user_prefs
    /// key into a `BTreeMap` for the snapshot body. Uses two SQL
    /// queries (one per prefix-or-exact match) rather than one
    /// big SELECT so the LIKE patterns stay simple and indexable.
    fn dump_settings(&self) -> SyncResult<BTreeMap<String, String>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut out = BTreeMap::new();
        for pattern in SYNC_WHITELIST {
            if pattern.ends_with('.') {
                // Prefix pattern — pull every key starting with it,
                // then drop the bare-prefix entry (which `is_synced_key`
                // would also reject).
                let mut stmt = conn
                    .prepare("SELECT key, value FROM user_prefs WHERE key LIKE ?")
                    .map_err(|err| SyncError::internal(format!("dump settings: {err}")))?;
                let like = format!("{pattern}%");
                let rows = stmt
                    .query_map(params![like], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| SyncError::internal(format!("dump settings: {err}")))?;
                for r in rows {
                    let (key, value) =
                        r.map_err(|err| SyncError::internal(format!("dump settings: {err}")))?;
                    if is_synced_key(&key) {
                        out.insert(key, value);
                    }
                }
            } else {
                // Exact key — single row.
                let value: Option<String> = conn
                    .query_row(
                        "SELECT value FROM user_prefs WHERE key = ?",
                        params![pattern],
                        |row| row.get(0),
                    )
                    .ok();
                if let Some(v) = value {
                    out.insert((*pattern).to_string(), v);
                }
            }
        }
        Ok(out)
    }
}

/// Insert-or-update the `accounts` table from a snapshot row.
/// Free function (rather than a method) so the apply loop can
/// call it under the existing db mutex without restructuring
/// the SnapshotBuilder borrow flow.
///
/// Skips the implicit `local` account — the schema's bootstrap
/// row already exists, and overwriting it with a snapshot copy
/// from another device would clobber its (locally-meaningful)
/// timestamps without any user-visible benefit.
fn upsert_snapshot_account(db: &SharedConn, acc: &SnapshotAccount) -> rusqlite::Result<()> {
    if acc.id == "local" {
        return Ok(());
    }
    let conn = db.lock().expect("db mutex poisoned");
    conn.execute(
        "INSERT INTO accounts
            (id, adapter_kind, display_name, config_json,
             created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            adapter_kind = excluded.adapter_kind,
            display_name = excluded.display_name,
            config_json  = excluded.config_json,
            updated_at   = excluded.updated_at",
        params![
            acc.id,
            acc.adapter_kind,
            acc.display_name,
            acc.config_json,
            acc.created_at,
            acc.updated_at,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use cal_core::{Calendar, ContainerColor};
    use tempfile::TempDir;

    fn fresh() -> (TempDir, DbHandle, Arc<LocalAdapter>) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        let adapter = Arc::new(LocalAdapter::new(db.shared()));
        (dir, db, adapter)
    }

    #[tokio::test]
    async fn build_then_apply_round_trips_rows_and_settings() {
        let (_tmp, db, adapter) = fresh();
        // Seed the source: one calendar + two whitelisted settings.
        adapter
            .upsert_calendar_from_sync(&Calendar {
                id: "cal-x".into(),
                name: "Test".into(),
                color: Some(ContainerColor::custom("#abcdef")),
                read_only: false,
                default_sound: None,
            })
            .unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        prefs.set("appearance.darkMode", "true").unwrap();
        prefs.set("locale", "de-DE").unwrap();
        // Non-whitelisted key — must not appear in the dump.
        prefs.set("sidebar.expansion", "{}").unwrap();

        let builder = SnapshotBuilder::new(db.shared(), adapter, "1.0.0-test");
        let snap = builder.build().unwrap();

        // The wire body should contain exactly the whitelisted keys
        // plus the one calendar.
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        assert_eq!(body.dump.calendars.len(), 1);
        assert_eq!(
            body.settings.get("appearance.darkMode").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            body.settings.get("locale").map(String::as_str),
            Some("de-DE")
        );
        assert!(!body.settings.contains_key("sidebar.expansion"));

        // Apply to a fresh device — the original DB has the data
        // already, so we use a second one.
        let (_tmp2, db2, adapter2) = fresh();
        let builder2 = SnapshotBuilder::new(db2.shared(), adapter2.clone(), "1.0.0-test");
        let outcome = builder2.apply(&snap).unwrap();
        assert_eq!(outcome.rows_applied, 1);
        assert_eq!(outcome.rows_failed, 0);
        assert_eq!(outcome.settings_applied, 2);
        assert_eq!(outcome.settings_failed, 0);

        // Settings landed in the destination's user_prefs.
        let shared2 = db2.shared();
        let prefs2 = UserPrefsRepo::new(&shared2);
        assert_eq!(
            prefs2.get("appearance.darkMode").unwrap().as_deref(),
            Some("true"),
        );
        assert_eq!(prefs2.get("locale").unwrap().as_deref(), Some("de-DE"));
        // The non-whitelisted key was never in the snapshot so
        // it stays absent.
        assert!(prefs2.get("sidebar.expansion").unwrap().is_none());
    }

    /// §19.11.8 — non-secret account rows round-trip from one
    /// device's snapshot to another. Verifies the `dump_accounts`
    /// → snapshot → `apply` chain lands the same row on the other
    /// side without touching the keychain. The implicit
    /// `id = "local"` account stays excluded from both sides.
    #[tokio::test]
    async fn accounts_round_trip_in_snapshot() {
        let (_tmp, db, adapter) = fresh();
        // Seed the source: insert one external account directly
        // via SQL (the test layer has no need to spin up the
        // adapter registry just to upsert a row).
        {
            let shared = db.shared();
            let conn = shared.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts
                    (id, adapter_kind, display_name, config_json,
                     created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    "acc-1",
                    "caldav",
                    "Fastmail",
                    r#"{"server_url":"https://caldav.fastmail.com/"}"#,
                    "2026-05-24T10:00:00Z",
                    "2026-05-24T10:00:00Z",
                ],
            )
            .unwrap();
        }

        let builder = SnapshotBuilder::new(db.shared(), adapter, "1.0.0-test");
        let snap = builder.build().unwrap();

        // The wire body should include exactly the one external
        // account row — the implicit `local` row is excluded.
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        assert_eq!(body.accounts.len(), 1);
        assert_eq!(body.accounts[0].id, "acc-1");
        assert_eq!(body.accounts[0].adapter_kind, "caldav");
        assert_eq!(body.accounts[0].display_name, "Fastmail");

        // Apply to a fresh device — should land the same row.
        let (_tmp2, db2, adapter2) = fresh();
        let builder2 = SnapshotBuilder::new(db2.shared(), adapter2.clone(), "1.0.0-test");
        let outcome = builder2.apply(&snap).unwrap();
        assert_eq!(outcome.accounts_applied, 1);
        assert_eq!(outcome.accounts_failed, 0);

        // Row landed in the destination.
        let shared2 = db2.shared();
        let conn2 = shared2.lock().unwrap();
        let (kind, name): (String, String) = conn2
            .query_row(
                "SELECT adapter_kind, display_name FROM accounts WHERE id = ?",
                params!["acc-1"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "caldav");
        assert_eq!(name, "Fastmail");
    }

    /// Local accounts (`id = "local"`) are never written into the
    /// snapshot — every device already has the bootstrap row, and
    /// overwriting it with someone else's timestamps would just
    /// flap the row on every onboarding.
    #[tokio::test]
    async fn snapshot_excludes_local_account() {
        let (_tmp, db, adapter) = fresh();
        // The bootstrap row for `id = "local"` is created by the
        // migrations, so we just verify it exists then build.
        {
            let shared = db.shared();
            let conn = shared.lock().unwrap();
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM accounts WHERE id = 'local'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "bootstrap local account should exist");
        }
        let builder = SnapshotBuilder::new(db.shared(), adapter, "1.0.0-test");
        let snap = builder.build().unwrap();
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        // No `local` row in the dumped accounts list.
        assert!(body.accounts.iter().all(|acc| acc.id != "local"));
    }

    #[tokio::test]
    async fn apply_drops_settings_not_on_whitelist() {
        // Forward-compat / hostile-peer guard: a snapshot body
        // claiming to write a non-synced key must be rejected.
        let (_tmp, db, adapter) = fresh();
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
        let builder = SnapshotBuilder::new(db.shared(), adapter, "1.0.0-test");
        let outcome = builder.apply(&snap).unwrap();
        assert_eq!(outcome.settings_applied, 1);
        assert_eq!(outcome.settings_failed, 1);
        // Confirm the protected key didn't land.
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        assert!(prefs.get("sync.deviceId").unwrap().is_none());
    }
}
