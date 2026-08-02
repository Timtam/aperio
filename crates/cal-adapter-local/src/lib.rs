//! Local calendar and task adapter backed by SQLite.
//!
//! This adapter is the simplest of the bunch: it owns no network code, no
//! OAuth flow, and no provider quirks. It exists for two reasons:
//!
//! 1. To give users a place for fully local calendars and task lists that
//!    never leave the device unless the user opts into the event-log sync
//!    (DESIGN.md section 6, "Lokale Kalender").
//! 2. To serve as the reference implementation of the [`cal_core`] adapter
//!    traits so the other adapters can be measured against the same shape.
//!
//! Migrations are intentionally **not** the adapter's concern. The caller
//! (the Tauri backend) opens the database, applies migrations, and hands
//! the connection in. The adapter assumes the schema from
//! `src-tauri/src/db/sql/0001_init.sql` is present.

mod calendars;
mod color_labels;
mod contacts;
mod mapping;
/// Mirroring the dataset into a filesystem directory — the store's sync half.
pub mod mirror;
mod search;
mod sync_apply;
mod sync_snapshot;
mod tasks;

pub use mirror::LocalFsSyncAdapter;
pub use search::{prepare_fts_query, EventTypeFilter, SearchFilters, SearchKind, SearchResults};
pub use sync_snapshot::{SnapshotApplyReport, SnapshotDump};

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cal_core::{
    Adapter, AdapterSource, AuthToken, Capability, Credentials, Error as CoreError, Result,
};
use rusqlite::Connection;

/// Identifier used as `AdapterSource` for all rows owned by this adapter.
pub const SOURCE_ID: &str = "local";

/// Shared SQLite connection handle.
///
/// We accept the connection from outside so the Tauri backend can keep a
/// single `Arc<Mutex<Connection>>` across all subsystems (settings,
/// adapters, sync, …). SQLite serialises writes anyway, so a single
/// connection under a mutex is the simplest correct choice.
pub type SharedConn = Arc<Mutex<Connection>>;

/// Provider of read-only connections that can run CONCURRENTLY with the
/// writer (the host's WAL read pool). Installed by the host after
/// construction; without one, reads fall back to the writer mutex (the
/// in-memory test databases have no pool). The provider MUST invoke the
/// closure exactly once.
pub trait ReadConnProvider: Send + Sync {
    fn with_read(&self, f: &mut dyn FnMut(&Connection));
}

/// Local-only calendar and task adapter.
pub struct LocalAdapter {
    db: SharedConn,
    /// WAL read pool for the pure-read paths — without it, a read issued
    /// while a sync-apply / warm-pass transaction holds the writer mutex
    /// waits the whole transaction out (visible as the first paint
    /// stalling behind the launch sync).
    read_pool: Option<Arc<dyn ReadConnProvider>>,
    source: AdapterSource,
    capabilities: Vec<Capability>,
}

impl LocalAdapter {
    /// Construct an adapter bound to an existing migrated database.
    pub fn new(db: SharedConn) -> Self {
        Self {
            db,
            read_pool: None,
            source: AdapterSource::new(SOURCE_ID),
            capabilities: vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts,
            ],
        }
    }

    /// Install the host's WAL read pool (see [`ReadConnProvider`]).
    pub fn with_read_pool(mut self, pool: Arc<dyn ReadConnProvider>) -> Self {
        self.read_pool = Some(pool);
        self
    }

    /// The source identifier used for every row this adapter owns.
    pub fn source(&self) -> &AdapterSource {
        &self.source
    }

    pub(crate) fn db(&self) -> &SharedConn {
        &self.db
    }

    /// Run a PURE READ on the pool when one is installed (concurrent with
    /// the writer under WAL), else on the writer mutex. Only for
    /// SELECT-only closures — anything that writes must keep using
    /// [`Self::db`].
    pub(crate) fn read<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        match &self.read_pool {
            Some(pool) => {
                let mut run = Some(f);
                let mut out: Option<R> = None;
                pool.with_read(&mut |conn| {
                    if let Some(f) = run.take() {
                        out = Some(f(conn));
                    }
                });
                out.expect("ReadConnProvider must invoke the closure")
            }
            None => f(&self.db.lock().expect("db mutex poisoned")),
        }
    }
}

#[async_trait]
impl Adapter for LocalAdapter {
    async fn authenticate(&self, _credentials: Credentials) -> Result<AuthToken> {
        // No remote auth — local calendars are always "authenticated".
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

/// Convert any `rusqlite::Error` into a `cal_core::Error::Internal` with the
/// original message preserved.
pub(crate) fn map_sql_err(err: rusqlite::Error) -> CoreError {
    CoreError::internal(err.to_string())
}

/// Convert a `serde_json` error into a `cal_core::Error::Internal`.
pub(crate) fn map_json_err(err: serde_json::Error) -> CoreError {
    CoreError::internal(format!("json: {err}"))
}

/// In-memory test fixtures.
///
/// Gated on the `test-support` feature flag so production builds
/// don't pull in `tempfile` etc., while still letting downstream
/// crates' integration tests (notably the event-log applier in
/// `src-tauri`) spin up a fully-migrated in-memory DB without
/// re-implementing the schema replay. Not part of the stable
/// public API — `#[doc(hidden)]` keeps it out of rustdoc.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support {
    use std::sync::{Arc, Mutex};

    use rusqlite::Connection;

    /// Open an in-memory connection migrated to the current schema.
    ///
    /// Brings a fresh in-memory DB up to date with the shared `aperio-db`
    /// migration runner — the exact runner a real desktop or mobile launch
    /// uses — so the test schema matches production and stays a single
    /// source of truth without a fragile cross-crate `include_str!` path.
    pub fn open_test_db() -> super::SharedConn {
        let mut conn = Connection::open_in_memory().expect("open in-memory");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable fk");
        aperio_db::run(&mut conn).expect("apply migrations");
        Arc::new(Mutex::new(conn))
    }
}
