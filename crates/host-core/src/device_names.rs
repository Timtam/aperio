//! Device names — this one's, and everybody else's.
//!
//! Two halves that sound alike and are not:
//!
//! - [`local_device_name`] / [`set_local_device_name`] own the name THIS
//!   device publishes. It is a device-local preference (`sync.deviceName`,
//!   deliberately not in `SYNC_WHITELIST` — a synced device name would give
//!   every device the same one), and the heartbeat writes it into this
//!   device's `meta.json` record on the next round.
//! - [`DeviceNamesRepo`] is a read-through CACHE of what the OTHER devices
//!   published, so a panel can say "announced by MacBook" without waiting for
//!   a fetch.
//!
//! ## The old note below is still true of the cache
//!
//! Local cache of every cross-device sync participant's
//! human-readable name (DESIGN.md §19 + §20.8).
//!
//! The cross-device `meta.json` holds a `DeviceRecord` per
//! participant; each record carries an optional `name`
//! ("Desktop-PC", "MacBook", …) set during onboarding. The
//! orchestrator upserts the names into this table after every
//! successful `fetch_meta` so the rest of the host can render
//! "Used on: MacBook" instead of a raw UUID without dragging
//! the orchestrator's lifecycle into every consumer.
//!
//! Only the current §20.8 "Plugin benötigt" panel reads from
//! here today. The §19 sync panel could pick it up too in a
//! follow-up.

use rusqlite::params;
use thiserror::Error;

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

pub use sync_engine::PREF_DEVICE_NAME;

/// The name this device publishes to the rest of the dataset, or `None` while
/// it has never been named.
///
/// `None` is not a failure and not an empty string: it means every other
/// device's list shows this one as a bare 32-character id, which is exactly
/// what the frontends offer to fix.
pub fn local_device_name(prefs: &UserPrefsRepo<'_>) -> Option<String> {
    prefs
        .get(PREF_DEVICE_NAME)
        .ok()
        .flatten()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Set the name this device publishes; a blank one clears it.
///
/// Takes effect on the next round rather than immediately — the heartbeat
/// compares the stored name against the published record and pushes when they
/// differ, so a rename is one meta write and needs nothing else to notice it.
///
/// Clearing rather than storing `""` is what keeps that comparison honest: the
/// record's name is an `Option`, so a stored empty string would read as a
/// change on every single round and never settle.
pub fn set_local_device_name(
    prefs: &UserPrefsRepo<'_>,
    name: &str,
) -> crate::user_prefs::UserPrefsResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        prefs.delete(PREF_DEVICE_NAME)
    } else {
        prefs.set(PREF_DEVICE_NAME, trimmed)
    }
}

#[derive(Debug, Error)]
pub enum DeviceNamesError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type DeviceNamesResult<T> = Result<T, DeviceNamesError>;

pub struct DeviceNamesRepo<'a> {
    db: &'a SharedConn,
}

impl<'a> DeviceNamesRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Upsert a single device's name. `None` is stored
    /// verbatim — the row still gets created so a future
    /// upsert can fill the name in once meta.json gets one.
    pub fn upsert(&self, device_id: &str, name: Option<&str>) -> DeviceNamesResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO device_names (device_id, name)
             VALUES (?, ?)
             ON CONFLICT(device_id) DO UPDATE SET name = excluded.name",
            params![device_id, name],
        )?;
        Ok(())
    }

    /// Lookup the cached name for `device_id`. Returns
    /// `Ok(None)` for unknown ids AND for ids whose
    /// DeviceRecord didn't carry a name.
    pub fn get(&self, device_id: &str) -> DeviceNamesResult<Option<String>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare("SELECT name FROM device_names WHERE device_id = ?")?;
        let row = stmt
            .query_row(params![device_id], |row| row.get::<_, Option<String>>(0))
            .ok()
            .flatten();
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }

    /// Blank clears rather than storing an empty string. The heartbeat
    /// compares its stored name against the published `Option<String>`, and a
    /// stored `""` would never equal `None` — so every round would read as a
    /// rename and push meta, forever.
    #[test]
    fn a_blank_name_clears_rather_than_storing_nothing() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        set_local_device_name(&prefs, "  Arbeitsrechner  ").unwrap();
        assert_eq!(
            local_device_name(&prefs).as_deref(),
            Some("Arbeitsrechner"),
            "surrounding space is not part of a name",
        );

        set_local_device_name(&prefs, "   ").unwrap();
        assert_eq!(local_device_name(&prefs), None);
        assert_eq!(prefs.get(PREF_DEVICE_NAME).unwrap(), None);
    }

    #[test]
    fn an_unnamed_device_reads_as_none() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        assert_eq!(local_device_name(&UserPrefsRepo::new(&shared)), None);
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = DeviceNamesRepo::new(&shared);
        repo.upsert("device-alpha", Some("MacBook")).unwrap();
        assert_eq!(
            repo.get("device-alpha").unwrap().as_deref(),
            Some("MacBook")
        );
    }

    #[test]
    fn unknown_device_returns_none() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = DeviceNamesRepo::new(&shared);
        assert!(repo.get("ghost").unwrap().is_none());
    }

    #[test]
    fn upsert_overwrites_existing_name() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = DeviceNamesRepo::new(&shared);
        repo.upsert("device-alpha", Some("Old")).unwrap();
        repo.upsert("device-alpha", Some("New")).unwrap();
        assert_eq!(repo.get("device-alpha").unwrap().as_deref(), Some("New"));
    }

    #[test]
    fn upsert_with_none_clears_name() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = DeviceNamesRepo::new(&shared);
        repo.upsert("device-alpha", Some("MacBook")).unwrap();
        repo.upsert("device-alpha", None).unwrap();
        assert!(repo.get("device-alpha").unwrap().is_none());
    }
}
