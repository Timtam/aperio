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

mod migrations;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
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

    fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Connection) -> R,
    {
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
            read_pool: None,
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

    /// Migration 0006 collapses the old `deadline_type` enum into the
    /// new (scheduled, deadline) pair. The data-preservation rules are
    /// documented in the migration header; this test pins all four
    /// branches in one shot.
    #[test]
    fn migration_0006_preserves_legacy_task_dates() {
        use rusqlite::params;

        // To exercise the migration on real legacy rows we build a
        // snapshot DB at v5 (apply 0001…0005 explicitly), seed the
        // legacy shape, then run 0006 and inspect the output.
        let legacy = Connection::open_in_memory().unwrap();
        legacy.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        legacy
            .execute_batch(include_str!("sql/0001_init.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0002_search.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0003_accounts.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0004_container_overrides.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0005_user_prefs.sql"))
            .unwrap();

        // Migration 0003 already seeds the 'local' account row; skip
        // re-inserting (the SQL we execute_batch'd ran every step).
        legacy
            .execute(
                "INSERT INTO task_lists (id, name, source, account_id, created_at, updated_at)
                 VALUES ('L1', 'List', 'local', 'local',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();

        // Four rows covering each branch of the migration.
        let mk = |id: &str,
                  sd: Option<&str>,
                  dt: Option<&str>,
                  dd: Option<&str>,
                  dtime: Option<&str>| {
            legacy
                .execute(
                    "INSERT INTO tasks (
                        id, list_id, title, status, priority,
                        scheduled_date, deadline_type, deadline_date, deadline_time,
                        created_at, updated_at
                     ) VALUES (?, 'L1', ?, 'open', 'medium', ?, ?, ?, ?,
                               '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    params![id, id, sd, dt, dd, dtime],
                )
                .unwrap();
        };

        // a: legacy "by" deadline — keep deadline as-is.
        mk("a", None, Some("by"), Some("2026-07-31"), None);
        // b: legacy "on" with no scheduled — move to scheduled.
        mk("b", None, Some("on"), Some("2026-08-15"), Some("14:30:00"));
        // c: legacy "on" with scheduled = same day — keep schedule, clear deadline.
        mk(
            "c",
            Some("2026-09-01"),
            Some("on"),
            Some("2026-09-01"),
            None,
        );
        // d: legacy "on" with scheduled = different day — keep both,
        // deadline degrades to "by".
        mk(
            "d",
            Some("2026-10-05"),
            Some("on"),
            Some("2026-10-10"),
            Some("17:00:00"),
        );

        // Apply 0006.
        legacy
            .execute_batch(include_str!("sql/0006_task_time_fields.sql"))
            .unwrap();

        let read = |id: &str| -> (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) {
            legacy
                .query_row(
                    "SELECT scheduled_date, scheduled_time, deadline_date, deadline_time
                       FROM tasks WHERE id = ?",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap()
        };

        // a — "by" preserved verbatim, schedule untouched.
        assert_eq!(
            read("a"),
            (None, None, Some("2026-07-31".to_string()), None)
        );
        // b — "on" without schedule → schedule takes the date+time.
        assert_eq!(
            read("b"),
            (
                Some("2026-08-15".to_string()),
                Some("14:30:00".to_string()),
                None,
                None
            )
        );
        // c — "on" same day as schedule → schedule kept, deadline cleared.
        assert_eq!(
            read("c"),
            (Some("2026-09-01".to_string()), None, None, None)
        );
        // d — "on" different day → both preserved (Plan + Soft-Deadline).
        assert_eq!(
            read("d"),
            (
                Some("2026-10-05".to_string()),
                None,
                Some("2026-10-10".to_string()),
                Some("17:00:00".to_string())
            )
        );
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
