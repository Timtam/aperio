//! Local SQLite layer.
//!
//! A single connection guarded by a [`Mutex`] is sufficient for a desktop
//! app: SQLite serialises writes anyway, the connection is shared across
//! all Tauri commands, and the simplicity beats a connection pool here.
//!
//! Migrations are tracked via the SQLite `user_version` pragma and run on
//! every [`DbHandle::open`] call (idempotent).

mod migrations;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use thiserror::Error;

pub use migrations::CURRENT_SCHEMA_VERSION;

/// A shared, mutex-guarded SQLite connection.
///
/// This is the canonical handle subsystems pass around. The Tauri backend
/// constructs exactly one of these and shares it with every subsystem
/// (calendar adapter, sync engine, plugin manager).
pub type SharedConn = Arc<Mutex<Connection>>;

/// Errors returned by the database layer.
///
/// The intent is to keep one variant per failure category that the caller
/// is expected to handle differently. Lower-level rusqlite errors are
/// flattened into [`DbError::Sqlite`] with the original message preserved.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(String),

    #[error("database migration failed (target version {target}): {message}")]
    Migration { target: u32, message: String },

    #[error("invariant violated: {0}")]
    Invariant(String),
}

impl From<rusqlite::Error> for DbError {
    fn from(value: rusqlite::Error) -> Self {
        DbError::Sqlite(value.to_string())
    }
}

pub type DbResult<T> = std::result::Result<T, DbError>;

/// Owned handle to the local database.
///
/// Wraps a [`SharedConn`] and adds convenience methods for borrowing the
/// connection and running write transactions. Migrations run once on
/// construction.
#[derive(Clone)]
pub struct DbHandle {
    conn: SharedConn,
}

impl DbHandle {
    /// Open a database file. Creates the file if it does not exist, applies
    /// PRAGMAs, and runs any pending migrations.
    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let conn = Connection::open(path.as_ref())?;
        Self::from_connection(conn)
    }

    /// In-memory connection. Used by tests and ephemeral scenarios.
    pub fn open_in_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> DbResult<Self> {
        // PRAGMAs we always want.
        // - foreign_keys=ON: enforce relations at write time.
        // - journal_mode=WAL: better concurrent-read behaviour with a single
        //   writer. Falls back automatically on read-only file systems.
        // - synchronous=NORMAL: durable enough for desktop use, much faster
        //   than FULL.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        let handle = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        migrations::run(&handle)?;
        Ok(handle)
    }

    /// Borrow the shared connection so it can be handed to subsystems that
    /// want their own `Arc` clone (e.g. `cal-adapter-local`).
    pub fn shared(&self) -> SharedConn {
        self.conn.clone()
    }

    /// Run a closure with a borrowed connection.
    ///
    /// The mutex guard is held for the duration of `f`. Keep closures short
    /// and never call `with_conn` recursively — the mutex is not re-entrant.
    pub fn with_conn<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Connection) -> R,
    {
        let guard = self.conn.lock().expect("db mutex poisoned");
        f(&guard)
    }

    /// Run a closure inside an exclusive write transaction.
    pub fn with_tx<F, R>(&self, f: F) -> DbResult<R>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> DbResult<R>,
    {
        let mut guard = self.conn.lock().expect("db mutex poisoned");
        let tx = guard.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_runs_migrations() {
        let db = DbHandle::open_in_memory().unwrap();
        let version: u32 = db.with_conn(|c| {
            c.query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap()
        });
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migrations_are_idempotent() {
        let db = DbHandle::open_in_memory().unwrap();
        // Running again on the same connection must not error.
        migrations::run(&db).unwrap();
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = DbHandle::open_in_memory().unwrap();
        let on: i32 = db.with_conn(|c| {
            c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(on, 1);
    }
}
