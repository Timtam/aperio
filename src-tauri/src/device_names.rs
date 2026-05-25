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
