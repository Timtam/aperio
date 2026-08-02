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
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use sync_engine::whitelist::{is_synced_key, SYNC_WHITELIST};
use sync_engine::{NewConflict, SnapshotAccount, StoreError, SyncStore};

use crate::conflicts::ConflictsRepo;
use crate::db::SharedConn;
use crate::remote_plugins::RemotePluginsRepo;
use crate::user_prefs::UserPrefsRepo;

/// The desktop SQLite-backed [`SyncStore`].
pub struct DesktopSyncStore {
    db: SharedConn,
    /// Used for the row dump/apply (`dump_for_snapshot` /
    /// `apply_snapshot_dump`). Points at the same `SharedConn` as `db`.
    adapter: Arc<LocalAdapter>,
    /// Answers whether an account belongs on the wire. See [`Self::travels`].
    plugins: Option<Arc<plugin_core::PluginManager>>,
}

impl DesktopSyncStore {
    pub fn new(db: SharedConn, adapter: Arc<LocalAdapter>) -> Self {
        Self {
            db,
            adapter,
            plugins: None,
        }
    }

    /// Give the store the plugin manager, so the snapshot can tell an account
    /// that travels from one that stays here.
    ///
    /// Optional because the snapshot is built on a path that has no manager in
    /// several tests, and because absent has a safe meaning: without it the
    /// only accounts filtered are the two host-internal kinds, which is what
    /// this did before the question existed. It is set on both hosts at
    /// start-up; a host that forgets publishes more than it should, so the
    /// wiring is not optional in practice, only in type.
    pub fn with_plugins(mut self, plugins: Arc<plugin_core::PluginManager>) -> Self {
        self.plugins = Some(plugins);
        self
    }

    /// Whether this account belongs in a snapshot.
    ///
    /// The event path asks `travels_between_devices` at every emitter. The
    /// snapshot is the other way out, and used to ask a narrower question —
    /// only the two host-internal names — so a sync target, once it became an
    /// account, would have been published: its address written in the clear
    /// into a file on the very server it names, and its password alongside when
    /// end-to-end encryption is on, because `dump_credentials` walks whatever
    /// this returns.
    fn travels(&self, adapter_kind: &str) -> bool {
        if sync_core::event::is_host_internal_kind(adapter_kind) {
            return false;
        }
        match self.plugins.as_deref() {
            Some(manager) => crate::accounts::travels_between_devices(manager, adapter_kind),
            None => true,
        }
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
                // Exact key — single row. `optional()` rather than `.ok()`:
                // the dump is published as the authoritative settings state,
                // so a key dropped by a failed read would look to every
                // onboarding device like a setting the user never chose. The
                // prefix branch above already propagates; this one has to say
                // the same thing rather than return an incomplete `Ok`.
                let value: Option<String> = conn
                    .query_row(
                        "SELECT value FROM user_prefs WHERE key = ?",
                        params![pattern],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|err| StoreError::Backend(format!("dump settings: {err}")))?;
                if let Some(v) = value {
                    out.insert((*pattern).to_string(), v);
                }
            }
        }
        Ok(out)
    }

    fn get_pref(&self, key: &str) -> Result<Option<String>, StoreError> {
        UserPrefsRepo::new(&self.db)
            .get(key)
            .map_err(|err| StoreError::Backend(format!("get pref: {err}")))
    }

    fn set_pref(&self, key: &str, value: &str) -> Result<(), StoreError> {
        UserPrefsRepo::new(&self.db)
            .set(key, value)
            .map_err(|err| StoreError::Backend(format!("set pref: {err}")))
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
            let account: SnapshotAccount =
                r.map_err(|err| StoreError::Backend(format!("dump accounts row: {err}")))?;
            if !self.travels(&account.adapter_kind) {
                continue;
            }
            out.push(account);
        }
        Ok(out)
    }

    /// Insert-or-update one account row from a snapshot. Skips the
    /// implicit `local` account — the schema's bootstrap row already
    /// exists, and overwriting it with a snapshot copy from another
    /// device would clobber its (locally-meaningful) timestamps without
    /// any user-visible benefit.
    fn upsert_account(&self, account: &SnapshotAccount) -> Result<(), StoreError> {
        // The same rule as the sending end's, because a snapshot written by an
        // older build — or by a device whose plugin set differs — carries rows
        // this one would otherwise trust.
        if account.id == "local" || !self.travels(&account.adapter_kind) {
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

    fn is_event_applied(&self, event_id: &str) -> Result<bool, StoreError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT 1 FROM sync_applied_events WHERE event_id = ?")
            .map_err(|err| StoreError::Backend(format!("is_event_applied: {err}")))?;
        Ok(stmt.query_row(params![event_id], |_| Ok(())).is_ok())
    }

    fn mark_event_applied(&self, event_id: &str) -> Result<(), StoreError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO sync_applied_events
                (event_id, applied_at) VALUES (?, ?)",
            params![event_id, now],
        )
        .map_err(|err| StoreError::Backend(format!("mark_event_applied: {err}")))?;
        Ok(())
    }

    fn record_conflict(&self, conflict: &NewConflict) -> Result<(), StoreError> {
        ConflictsRepo::new(&self.db)
            .record(conflict.clone())
            .map(|_| ())
            .map_err(|err| StoreError::Backend(format!("record_conflict: {err}")))
    }

    fn delete_pref(&self, key: &str) -> Result<(), StoreError> {
        UserPrefsRepo::new(&self.db)
            .delete(key)
            .map_err(|err| StoreError::Backend(format!("delete_pref: {err}")))
    }

    fn delete_account(&self, id: &str) -> Result<(), StoreError> {
        if id == "local" {
            return Ok(());
        }
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM accounts WHERE id = ?", params![id])
            .map_err(|err| StoreError::Backend(format!("delete_account: {err}")))?;
        Ok(())
    }

    fn upsert_remote_plugin(
        &self,
        id: &str,
        name: Option<&str>,
        version: &str,
        plugin_type: Option<&str>,
        source: Option<&str>,
        announced_by_device: &str,
    ) -> Result<(), StoreError> {
        RemotePluginsRepo::new(&self.db)
            .upsert(id, name, version, plugin_type, source, announced_by_device)
            .map_err(|err| StoreError::Backend(format!("upsert_remote_plugin: {err}")))
    }

    fn delete_remote_plugin(&self, id: &str) -> Result<(), StoreError> {
        RemotePluginsRepo::new(&self.db)
            .delete(id)
            .map_err(|err| StoreError::Backend(format!("delete_remote_plugin: {err}")))
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

    /// A sync target's address must not be written into a file on the very
    /// server it names.
    ///
    /// The event path refuses it; the snapshot asks the same question only if
    /// it was given the plugin manager, and this asserts it was. With
    /// encryption on it matters twice over: `dump_credentials` walks whatever
    /// `dump_accounts` returns, so the target's own password would go with it.
    #[tokio::test]
    async fn snapshot_excludes_a_sync_only_account() {
        // The shipped WebDAV plugin — a real sync-ONLY adapter, kind read off
        // its own manifest so a rename cannot turn this into a test of the
        // unknown-kind branch. (Folder sync used to serve here; it folded into
        // the built-in store, whose account holds calendars and therefore does
        // not answer the question this test asks.)
        let manifest = plugin_core::manifest::PluginManifest::from_bytes(include_bytes!(
            "../../../sync-adapter-webdav-plugin/plugin.json"
        ))
        .expect("the shipped WebDAV sync manifest parses");
        let sync_kind = manifest
            .adapter_kind
            .clone()
            .expect("the sync manifest declares a kind");
        let plugins = Arc::new(plugin_core::PluginManager::new("0.1.0"));
        let descriptor = unsafe { sync_adapter_webdav_plugin::build_descriptor() };
        plugins
            .register_static(manifest, descriptor, sync_adapter_webdav_plugin::DESTROY_FN)
            .expect("register the static WebDAV sync plugin");

        let (_tmp, db, adapter) = fresh();
        {
            let shared = db.shared();
            let conn = shared.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts
                    (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('folder', ?1, 'Sicherung', '{\"remote_root\":\"/srv/aperio\"}',
                         '2026-01-01', '2026-01-01')",
                rusqlite::params![sync_kind],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO accounts
                    (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('fm', 'caldav', 'Fastmail', '{}', '2026-01-01', '2026-01-01')",
                [],
            )
            .unwrap();
        }

        let store = DesktopSyncStore::new(db.shared(), Arc::clone(&adapter)).with_plugins(plugins);
        let dumped = store.dump_accounts().expect("dump");

        assert!(
            dumped.iter().all(|a| a.adapter_kind != sync_kind),
            "a sync target reached the snapshot: {:?}",
            dumped.iter().map(|a| &a.id).collect::<Vec<_>>(),
        );
        assert!(
            dumped.iter().any(|a| a.id == "fm"),
            "an ordinary account must still travel",
        );
    }

    /// A phone's own calendar store is not something another device can open.
    ///
    /// The event path already refused it; the snapshot filtered by id alone
    /// and published it anyway, so it arrived on the desktop as an account
    /// with no plugin, asking to be reconnected. This asserts the sending end;
    /// `upsert_account` and the applier refuse it on arrival too, because a
    /// snapshot written by an older build still carries it.
    #[tokio::test]
    async fn snapshot_excludes_a_device_local_account() {
        let (_tmp, db, adapter) = fresh();
        {
            let shared = db.shared();
            let conn = shared.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts
                    (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('phone', 'device_calendar', 'Telefon', '{}', '2026-01-01', '2026-01-01')",
                [],
            )
            .unwrap();
            // A normal account beside it, so the test can tell "filtered" from
            // "dumped nothing at all".
            conn.execute(
                "INSERT INTO accounts
                    (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('fm', 'caldav', 'Fastmail', '{}', '2026-01-01', '2026-01-01')",
                [],
            )
            .unwrap();
        }
        let snap = builder(&db, Arc::clone(&adapter)).build().unwrap();
        let body: AperioSnapshotBody = serde_json::from_value(snap.body.clone()).unwrap();
        assert!(
            body.accounts
                .iter()
                .all(|acc| acc.adapter_kind != "device_calendar"),
            "a device-local account reached the snapshot: {:?}",
            body.accounts.iter().map(|a| &a.id).collect::<Vec<_>>(),
        );
        assert!(
            body.accounts.iter().any(|acc| acc.id == "fm"),
            "the ordinary account must still travel",
        );
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
