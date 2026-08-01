//! Generic user-preferences store.
//!
//! Wraps the `user_prefs` SQLite table behind a key/value API.
//! Values are opaque strings — callers serialise JSON if they need
//! structure. This is intentionally minimal so future features can
//! reuse the same table without bespoke schema for each.
//!
//! Current consumers:
//!
//!   - `sidebar.expansion` — tree-node expansion state for the
//!     account-grouped sidebar (Phase 6 follow-up).
//!
//! The repo deliberately does not interpret values — that's the
//! caller's job. Keeping serialisation out of the storage layer
//! means the same table can hold JSON, plain strings, base64, …
//! without growing schema variants.

use chrono::Utc;
use rusqlite::params;
use thiserror::Error;

use crate::db::SharedConn;

#[derive(Debug, Error)]
pub enum UserPrefsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type UserPrefsResult<T> = Result<T, UserPrefsError>;

pub struct UserPrefsRepo<'a> {
    pub(crate) db: &'a SharedConn,
}

impl<'a> UserPrefsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Read the value for `key`. Returns `None` when no row exists
    /// — the caller decides what the default behaviour should be.
    ///
    /// `None` means exactly that: no row. It used to also mean "the read
    /// failed", because the query ended in `.ok()`, and every failure a locked
    /// or damaged database can produce arrived as an unset preference. Callers
    /// cannot tell those apart and reasonably assume the first, so a moment of
    /// contention read as a user who had never chosen anything — and code that
    /// writes a default back in that case would have made it permanent.
    pub fn get(&self, key: &str) -> UserPrefsResult<Option<String>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare("SELECT value FROM user_prefs WHERE key = ?")?;
        match stmt.query_row(params![key], |row| row.get::<_, String>(0)) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Upsert a key/value pair. We use the SQLite `ON CONFLICT … DO
    /// UPDATE` idiom so the read API stays "last write wins" — no
    /// need for the caller to know whether a row exists.
    pub fn set(&self, key: &str, value: &str) -> UserPrefsResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO user_prefs (key, value, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
            params![key, value, now],
        )?;
        Ok(())
    }

    /// Drop the row for `key`. No-op when nothing matches; the
    /// caller wanted "make sure this is gone" and that's what they
    /// got either way.
    pub fn delete(&self, key: &str) -> UserPrefsResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM user_prefs WHERE key = ?", params![key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn unset_key_returns_none() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = UserPrefsRepo::new(&shared);
        assert!(repo.get("anything").unwrap().is_none());
    }

    #[test]
    fn set_then_get_roundtrips() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = UserPrefsRepo::new(&shared);
        repo.set("sidebar.expansion", r#"{"foo":true}"#).unwrap();
        assert_eq!(
            repo.get("sidebar.expansion").unwrap().as_deref(),
            Some(r#"{"foo":true}"#),
        );
    }

    #[test]
    fn set_overwrites_existing_value() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = UserPrefsRepo::new(&shared);
        repo.set("k", "v1").unwrap();
        repo.set("k", "v2").unwrap();
        assert_eq!(repo.get("k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn delete_clears_the_row() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = UserPrefsRepo::new(&shared);
        repo.set("k", "v").unwrap();
        repo.delete("k").unwrap();
        assert!(repo.get("k").unwrap().is_none());
    }

    #[test]
    fn delete_of_unset_key_is_noop() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = UserPrefsRepo::new(&shared);
        // Should not error.
        repo.delete("never-set").unwrap();
    }
}
