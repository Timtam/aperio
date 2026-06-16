//! Desktop implementation of `sync_engine::SyncStore`.
//!
//! Wraps the local SQLite: `LocalAdapter` for the calendar / event /
//! task / list / label rows, `UserPrefsRepo` + ad-hoc SQL for the
//! whitelisted settings and the non-secret account rows, and the
//! `credential_sync` E2E flag. This is what lets the platform-agnostic
//! `SnapshotBuilder` (and, later, the applier / compactor / orchestrator)
//! run unchanged on the desktop; the mobile target provides its own impl
//! over the same `LocalAdapter` plus its sandbox storage.
//!
//! The SQL here was moved verbatim out of the old
//! `event_log::snapshot::SnapshotBuilder` so behaviour is preserved — the
//! only change is the error type (`StoreError` instead of `SyncError`,
//! mapped back to `SyncError::internal` by the builder).

use std::collections::BTreeMap;
use std::sync::Arc;

use cal_adapter_local::{LocalAdapter, SnapshotApplyReport, SnapshotDump};
use rusqlite::params;
use sync_engine::whitelist::{is_synced_key, SYNC_WHITELIST};
use sync_engine::{SnapshotAccount, StoreError, SyncStore};

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// The desktop SQLite-backed [`SyncStore`].
pub struct DesktopSyncStore {
    db: SharedConn,
    /// Used for the row dump/apply (`dump_for_snapshot` /
    /// `apply_snapshot_dump`). Points at the same `SharedConn` as `db`.
    adapter: Arc<LocalAdapter>,
}

impl DesktopSyncStore {
    pub fn new(db: SharedConn, adapter: Arc<LocalAdapter>) -> Self {
        Self { db, adapter }
    }
}

impl SyncStore for DesktopSyncStore {
    fn dump_for_snapshot(&self) -> Result<SnapshotDump, StoreError> {
        self.adapter
            .dump_for_snapshot()
            .map_err(|err| StoreError::Backend(format!("dump rows: {err}")))
    }

    fn apply_snapshot_dump(&self, dump: &SnapshotDump) -> Result<SnapshotApplyReport, StoreError> {
        self.adapter
            .apply_snapshot_dump(dump)
            .map_err(|err| StoreError::Backend(format!("apply rows: {err}")))
    }

    /// Read the current values of every whitelisted user_prefs key into a
    /// `BTreeMap`. Uses two SQL queries (one per prefix-or-exact match)
    /// rather than one big SELECT so the LIKE patterns stay simple and
    /// indexable.
    fn dump_synced_settings(&self) -> Result<BTreeMap<String, String>, StoreError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut out = BTreeMap::new();
        for pattern in SYNC_WHITELIST {
            if pattern.ends_with('.') {
                // Prefix pattern — pull every key starting with it,
                // then drop the bare-prefix entry (which `is_synced_key`
                // would also reject).
                let mut stmt = conn
                    .prepare("SELECT key, value FROM user_prefs WHERE key LIKE ?")
                    .map_err(|err| StoreError::Backend(format!("dump settings: {err}")))?;
                let like = format!("{pattern}%");
                let rows = stmt
                    .query_map(params![like], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| StoreError::Backend(format!("dump settings: {err}")))?;
                for r in rows {
                    let (key, value) =
                        r.map_err(|err| StoreError::Backend(format!("dump settings: {err}")))?;
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

    fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        UserPrefsRepo::new(&self.db)
            .set(key, value)
            .map_err(|err| StoreError::Backend(format!("set setting: {err}")))
    }

    /// Read every account row except the implicit `local` one. The
    /// `local` account is skipped because the schema's bootstrap row
    /// recreates it on every device; including it would just bloat the
    /// snapshot. Secrets are NOT touched here — only the non-secret
    /// columns ride along (credentials stay in the OS keychain).
    fn dump_accounts(&self) -> Result<Vec<SnapshotAccount>, StoreError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, adapter_kind, display_name, config_json,
                        created_at, updated_at
                   FROM accounts
                  WHERE id != 'local'
                  ORDER BY display_name COLLATE NOCASE",
            )
            .map_err(|err| StoreError::Backend(format!("dump accounts prepare: {err}")))?;
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
            .map_err(|err| StoreError::Backend(format!("dump accounts query: {err}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|err| StoreError::Backend(format!("dump accounts row: {err}")))?);
        }
        Ok(out)
    }

    /// Insert-or-update one account row from a snapshot. Skips the
    /// implicit `local` account — the schema's bootstrap row already
    /// exists, and overwriting it with a snapshot copy from another
    /// device would clobber its (locally-meaningful) timestamps without
    /// any user-visible benefit.
    fn upsert_account(&self, account: &SnapshotAccount) -> Result<(), StoreError> {
        if account.id == "local" {
            return Ok(());
        }
        let conn = self.db.lock().expect("db mutex poisoned");
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
                account.id,
                account.adapter_kind,
                account.display_name,
                account.config_json,
                account.created_at,
                account.updated_at,
            ],
        )
        .map_err(|err| StoreError::Backend(format!("upsert account: {err}")))?;
        Ok(())
    }

    fn e2e_enabled(&self) -> bool {
        crate::credential_sync::e2e_enabled(&self.db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use sync_core::Snapshot;
    use sync_engine::{AperioSnapshotBody, SecretError, SecretSlot, SnapshotBuilder};

    use cal_core::{Calendar, ContainerColor};
    use chrono::Utc;
    use tempfile::TempDir;

    /// No-op [`SecretStore`] double. The round-trip tests below never
    /// enable E2E, so the builder never reads or writes a secret — but a
    /// `SnapshotBuilder` still needs *some* secret store, and we don't
    /// want tests touching the real OS keychain.
    #[derive(Default)]
    struct NoopSecrets;

    impl sync_engine::SecretStore for NoopSecrets {
        fn store(&self, _: &str, _: SecretSlot, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
        fn retrieve(&self, _: &str, _: SecretSlot) -> Result<String, SecretError> {
            Err(SecretError::NotFound)
        }
        fn delete(&self, _: &str, _: SecretSlot) -> Result<(), SecretError> {
            Ok(())
        }
        fn delete_all(&self, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
    }

    fn fresh() -> (TempDir, DbHandle, Arc<LocalAdapter>) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        let adapter = Arc::new(LocalAdapter::new(db.shared()));
        (dir, db, adapter)
    }

    fn builder(db: &DbHandle, adapter: Arc<LocalAdapter>) -> SnapshotBuilder {
        let store = Arc::new(DesktopSyncStore::new(db.shared(), adapter));
        SnapshotBuilder::new(store, Arc::new(NoopSecrets), "1.0.0-test")
    }

    #[tokio::test]
    async fn build_then_apply_round_trips_rows_and_settings() {
        let (_tmp, db, adapter) = fresh();
        // Seed the source: one calendar + two whitelisted settings.
        adapter
            .upsert_calendar_from_sync(&Calendar {
                color_label: None,
                supports_scheduling: false,
                supports_event_color: false,
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

        let snap = builder(&db, Arc::clone(&adapter)).build().unwrap();

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
        let outcome = builder(&db2, adapter2).apply(&snap).unwrap();
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

    /// §19.11.8 — non-secret account rows round-trip from one device's
    /// snapshot to another. Verifies the `dump_accounts` → snapshot →
    /// `apply` chain lands the same row on the other side without
    /// touching the keychain. The implicit `id = "local"` account stays
    /// excluded from both sides.
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

        let snap = builder(&db, Arc::clone(&adapter)).build().unwrap();

        // The wire body should include exactly the one external
        // account row — the implicit `local` row is excluded.
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        assert_eq!(body.accounts.len(), 1);
        assert_eq!(body.accounts[0].id, "acc-1");
        assert_eq!(body.accounts[0].adapter_kind, "caldav");
        assert_eq!(body.accounts[0].display_name, "Fastmail");

        // Apply to a fresh device — should land the same row.
        let (_tmp2, db2, adapter2) = fresh();
        let outcome = builder(&db2, adapter2).apply(&snap).unwrap();
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
        let snap = builder(&db, Arc::clone(&adapter)).build().unwrap();
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        // No `local` row in the dumped accounts list.
        assert!(body.accounts.iter().all(|acc| acc.id != "local"));
    }

    /// Hostile-peer guard at the desktop layer: a snapshot body claiming
    /// to write a non-synced key must be dropped, and the protected key
    /// must never land in `user_prefs`. (The builder-level version of
    /// this lives in sync-engine against a fake store; this one proves
    /// the real `DesktopSyncStore::set_setting` path agrees.)
    #[tokio::test]
    async fn apply_drops_settings_not_on_whitelist() {
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
        let outcome = builder(&db, adapter).apply(&snap).unwrap();
        assert_eq!(outcome.settings_applied, 1);
        assert_eq!(outcome.settings_failed, 1);
        // Confirm the protected key didn't land.
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        assert!(prefs.get("sync.deviceId").unwrap().is_none());
    }
}
