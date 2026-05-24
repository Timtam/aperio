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
use chrono::Utc;
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
    pub fn new(
        db: SharedConn,
        adapter: Arc<LocalAdapter>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            db,
            adapter,
            app_version: app_version.into(),
        }
    }

    /// Build a [`Snapshot`] reflecting the current local SQLite
    /// state + whitelisted user_prefs. `snapshot_timestamp` is set
    /// to `Utc::now()`; the caller is responsible for atomically
    /// updating `meta.json.snapshot_timestamp` to match after a
    /// successful `push_snapshot`.
    pub fn build(&self) -> SyncResult<Snapshot> {
        let dump = self
            .adapter
            .dump_for_snapshot()
            .map_err(|err| SyncError::internal(format!("dump rows: {err}")))?;
        let settings = self.dump_settings()?;
        let body = AperioSnapshotBody { dump, settings };
        let body_value = serde_json::to_value(&body)?;
        Ok(Snapshot::new(Utc::now(), self.app_version.clone(), body_value))
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
        let body: AperioSnapshotBody =
            serde_json::from_value(snapshot.body.clone())?;
        let mut outcome = SnapshotApplyOutcome::default();
        let report = self
            .adapter
            .apply_snapshot_dump(&body.dump)
            .map_err(|err| {
                SyncError::internal(format!("apply rows: {err}"))
            })?;
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
        Ok(outcome)
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
                    .map_err(|err| {
                        SyncError::internal(format!("dump settings: {err}"))
                    })?;
                let like = format!("{pattern}%");
                let rows = stmt
                    .query_map(params![like], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| {
                        SyncError::internal(format!("dump settings: {err}"))
                    })?;
                for r in rows {
                    let (key, value) = r.map_err(|err| {
                        SyncError::internal(format!("dump settings: {err}"))
                    })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use cal_core::{Calendar, ContainerColor};
    use tempfile::TempDir;

    fn fresh() -> (TempDir, DbHandle, Arc<LocalAdapter>) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(&dir.path().join("test.sqlite")).unwrap();
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
        let body: AperioSnapshotBody =
            serde_json::from_value(snap.body.clone()).unwrap();
        assert_eq!(body.dump.calendars.len(), 1);
        assert_eq!(body.settings.get("appearance.darkMode").map(String::as_str), Some("true"));
        assert_eq!(body.settings.get("locale").map(String::as_str), Some("de-DE"));
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
