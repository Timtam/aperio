//! Local SQLite layer.
//!
//! Writes go through a single connection guarded by a [`Mutex`] (SQLite
//! serialises writes anyway). Hot cache reads (`get_events` etc.) instead
//! use a small pool of read-only connections via
//! [`DbHandle::with_read_conn`] — with WAL those read concurrently with the
//! writer, so a view never stalls behind a background write such as the
//! startup cache warm pass.
//!
//! Migrations are tracked via the SQLite `user_version` pragma and run on
//! every [`DbHandle::open`] call (idempotent).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use thiserror::Error;

/// The local SQLite schema + migration runner live in the shared
/// `aperio-db` crate (so the desktop backend and the mobile app run the
/// same migrations); re-exported so existing `crate::db::CURRENT_SCHEMA_VERSION`
/// references keep resolving.
pub use aperio_db::CURRENT_SCHEMA_VERSION;

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

impl From<aperio_db::MigrationError> for DbError {
    fn from(value: aperio_db::MigrationError) -> Self {
        match value {
            aperio_db::MigrationError::Failed { target, message } => {
                DbError::Migration { target, message }
            }
            aperio_db::MigrationError::Sqlite(err) => DbError::Sqlite(err.to_string()),
        }
    }
}

pub type DbResult<T> = std::result::Result<T, DbError>;

/// Number of read-only connections in the pool (see [`ReadPool`]).
const READ_POOL_SIZE: usize = 4;

/// A small pool of read-only SQLite connections to the same WAL database
/// file, handed out round-robin. Each is guarded by its own mutex; with
/// WAL, reads run concurrently with each other and with the single writer,
/// so cache reads never serialise behind a write.
struct ReadPool {
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl ReadPool {
    /// Open `size` read-only connections to `path`. Must run AFTER the
    /// writer has set WAL + applied migrations (so the schema and the
    /// `-wal`/`-shm` files exist).
    fn open(path: &Path, size: usize) -> DbResult<Self> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open(path)?;
            // `query_only` rejects writes (a stray write routed here fails
            // loudly instead of racing the writer); `busy_timeout` waits
            // out the rare WAL checkpoint instead of erroring.
            conn.execute_batch(
                "PRAGMA busy_timeout = 5000;
                 PRAGMA query_only = ON;",
            )?;
            conns.push(Mutex::new(conn));
        }
        Ok(Self {
            conns,
            next: AtomicUsize::new(0),
        })
    }

    /// Run `f` on a pool connection. Prefers a FREE connection (try_lock
    /// scan) so one slow read can't head-of-line-block later reads that
    /// happen to round-robin onto its connection while others sit idle;
    /// only when every connection is busy does it block on the
    /// round-robin pick.
    ///
    /// NOT re-entrant: a nested `with` from inside `f` can land on the
    /// connection the thread already holds and deadlock (std Mutex).
    /// Callers — including `LocalAdapter::read` bodies — must never call
    /// back into the pool.
    fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Connection) -> R,
    {
        for conn in &self.conns {
            if let Ok(guard) = conn.try_lock() {
                return f(&guard);
            }
        }
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        let guard = self.conns[idx].lock().expect("read conn poisoned");
        f(&guard)
    }
}

/// Owned handle to the local database.
///
/// Wraps the writer [`SharedConn`] plus a read-only [`ReadPool`], and adds
/// convenience methods for borrowing them. Migrations run once on
/// construction.
#[derive(Clone)]
pub struct DbHandle {
    conn: SharedConn,
    /// Read-only pool for concurrent cache reads. `None` for in-memory
    /// databases (tests): `:memory:` can't be reopened as a second
    /// connection, so reads fall back to the writer connection.
    read_pool: Option<Arc<ReadPool>>,
}

impl DbHandle {
    /// Open a database file. Creates the file if it does not exist, applies
    /// PRAGMAs, and runs any pending migrations.
    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path)?;
        let mut handle = Self::from_connection(conn)?;
        // WAL + migrations are in place now, so the read pool can attach.
        handle.read_pool = Some(Arc::new(ReadPool::open(path, READ_POOL_SIZE)?));
        Ok(handle)
    }

    /// In-memory connection. Used by tests and ephemeral scenarios.
    pub fn open_in_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(mut conn: Connection) -> DbResult<Self> {
        // PRAGMAs we always want.
        // - foreign_keys=ON: enforce relations at write time.
        // - journal_mode=WAL: better concurrent-read behaviour with a single
        //   writer. Falls back automatically on read-only file systems.
        // - synchronous=NORMAL: durable enough for desktop use, much faster
        //   than FULL.
        // - busy_timeout: the same 5s wait the read pool takes. In-process
        //   contention serialises on the `SharedConn` mutex and never reaches
        //   SQLite, but a SECOND holder of the file — another app instance, a
        //   backup/AV scanner on Windows, a non-passive checkpoint — does, and
        //   without a busy handler every one of those returns SQLITE_BUSY
        //   instantly. Callers then have to decide what a momentary lock means
        //   for them; waiting it out means most never have to.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;

        // Bring the schema up to date BEFORE sharing the connection — the
        // adapter-local / sync-engine layers assume a migrated DB. The
        // runner lives in the shared `aperio-db` crate (so desktop and
        // mobile run the same migrations).
        aperio_db::run(&mut conn)?;

        let handle = Self {
            conn: Arc::new(Mutex::new(conn)),
            read_pool: None,
        };
        Ok(handle)
    }

    /// Borrow the shared connection so it can be handed to subsystems that
    /// want their own `Arc` clone (e.g. `adapter-local`).
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

    /// Run a closure with a borrowed READ-ONLY connection.
    ///
    /// Routes through the [`ReadPool`] (concurrent with the writer and
    /// other readers under WAL) when present, falling back to the writer
    /// connection for in-memory databases. SELECT-only — writes must use
    /// [`Self::with_conn`] / [`Self::with_tx`].
    pub fn with_read_conn<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Connection) -> R,
    {
        match &self.read_pool {
            Some(pool) => pool.with(f),
            None => self.with_conn(f),
        }
    }
}

/// Lets the local adapter's pure-read paths run on the WAL read pool
/// (concurrent with the writer) instead of waiting out whatever
/// transaction currently holds the writer mutex — at app start that is
/// the launch sync's apply writes, which used to stall the first paint's
/// local event/task reads.
impl adapter_local::ReadConnProvider for DbHandle {
    fn with_read(&self, f: &mut dyn FnMut(&rusqlite::Connection)) {
        self.with_read_conn(|c| f(c));
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
    fn foreign_keys_are_enforced() {
        let db = DbHandle::open_in_memory().unwrap();
        let on: i32 = db.with_conn(|c| {
            c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(on, 1);
    }

    #[test]
    fn read_pool_serves_committed_writes_on_a_file_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbHandle::open(tmp.path().join("pool.sqlite")).unwrap();
        // A file-based DB gets a real read pool (in-memory does not).
        assert!(db.read_pool.is_some());
        db.with_tx(|tx| {
            tx.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", [])?;
            tx.execute("INSERT INTO t (v) VALUES ('hello')", [])?;
            Ok(())
        })
        .unwrap();
        // The read pool sees the committed write.
        let v: String = db.with_read_conn(|c| {
            c.query_row("SELECT v FROM t WHERE id = 1", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(v, "hello");
    }

    #[test]
    fn read_pool_rejects_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbHandle::open(tmp.path().join("ro.sqlite")).unwrap();
        db.with_tx(|tx| {
            tx.execute("CREATE TABLE t (id INTEGER)", [])?;
            Ok(())
        })
        .unwrap();
        // query_only=ON → a write routed to a read connection errors out.
        let res = db.with_read_conn(|c| c.execute("INSERT INTO t (id) VALUES (1)", []));
        assert!(res.is_err(), "read pool must reject writes (query_only)");
    }

    #[test]
    fn in_memory_falls_back_to_writer_for_reads() {
        let db = DbHandle::open_in_memory().unwrap();
        assert!(db.read_pool.is_none());
        let one: i64 = db.with_read_conn(|c| c.query_row("SELECT 1", [], |r| r.get(0)).unwrap());
        assert_eq!(one, 1);
    }
}
