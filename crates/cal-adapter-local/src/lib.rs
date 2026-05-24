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
mod search;
mod sync_apply;
mod sync_snapshot;
mod tasks;

pub use search::{EventTypeFilter, SearchFilters, SearchKind, SearchResults};
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

/// Local-only calendar and task adapter.
pub struct LocalAdapter {
    db: SharedConn,
    source: AdapterSource,
    capabilities: Vec<Capability>,
}

impl LocalAdapter {
    /// Construct an adapter bound to an existing migrated database.
    pub fn new(db: SharedConn) -> Self {
        Self {
            db,
            source: AdapterSource::new(SOURCE_ID),
            capabilities: vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts,
            ],
        }
    }

    /// The source identifier used for every row this adapter owns.
    pub fn source(&self) -> &AdapterSource {
        &self.source
    }

    pub(crate) fn db(&self) -> &SharedConn {
        &self.db
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
    //! Helpers for unit tests.

    use std::sync::{Arc, Mutex};

    use rusqlite::Connection;

    /// Open an in-memory connection populated with the Phase-1 schema.
    ///
    /// The schema SQL is included from the Tauri backend so this crate
    /// stays free of a hard dependency on `aperio` while the schema text
    /// remains a single source of truth.
    pub fn open_test_db() -> super::SharedConn {
        let conn = Connection::open_in_memory().expect("open in-memory");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable fk");
        // Replay every migration so the in-memory schema matches what
        // a real Tauri-launched DB sees after the migration runner has
        // caught up. Add new SCHEMA_V<N> consts alongside the SQL files
        // — keeping the lists in sync is checked indirectly by the
        // tests that hit the columns the latest migration introduces.
        conn.execute_batch(SCHEMA_V1).expect("apply v1 schema");
        conn.execute_batch(SCHEMA_V2).expect("apply v2 schema");
        conn.execute_batch(SCHEMA_V3).expect("apply v3 schema");
        conn.execute_batch(SCHEMA_V4).expect("apply v4 schema");
        conn.execute_batch(SCHEMA_V5).expect("apply v5 schema");
        conn.execute_batch(SCHEMA_V6).expect("apply v6 schema");
        conn.execute_batch(SCHEMA_V7).expect("apply v7 schema");
        conn.execute_batch(SCHEMA_V8).expect("apply v8 schema");
        conn.execute_batch(SCHEMA_V9).expect("apply v9 schema");
        conn.execute_batch(SCHEMA_V10).expect("apply v10 schema");
        conn.execute_batch(SCHEMA_V11).expect("apply v11 schema");
        conn.execute_batch(SCHEMA_V12).expect("apply v12 schema");
        conn.execute_batch(SCHEMA_V13).expect("apply v13 schema");
        Arc::new(Mutex::new(conn))
    }

    const SCHEMA_V1: &str = include_str!("../../../src-tauri/src/db/sql/0001_init.sql");
    const SCHEMA_V2: &str = include_str!("../../../src-tauri/src/db/sql/0002_search.sql");
    const SCHEMA_V3: &str = include_str!("../../../src-tauri/src/db/sql/0003_accounts.sql");
    const SCHEMA_V4: &str =
        include_str!("../../../src-tauri/src/db/sql/0004_container_overrides.sql");
    const SCHEMA_V5: &str = include_str!("../../../src-tauri/src/db/sql/0005_user_prefs.sql");
    const SCHEMA_V6: &str =
        include_str!("../../../src-tauri/src/db/sql/0006_task_time_fields.sql");
    const SCHEMA_V7: &str = include_str!("../../../src-tauri/src/db/sql/0007_contacts.sql");
    const SCHEMA_V8: &str =
        include_str!("../../../src-tauri/src/db/sql/0008_contacts_fts.sql");
    const SCHEMA_V9: &str =
        include_str!("../../../src-tauri/src/db/sql/0009_contact_members.sql");
    const SCHEMA_V10: &str =
        include_str!("../../../src-tauri/src/db/sql/0010_contact_photos.sql");
    const SCHEMA_V11: &str =
        include_str!("../../../src-tauri/src/db/sql/0011_contact_addresses.sql");
    const SCHEMA_V12: &str =
        include_str!("../../../src-tauri/src/db/sql/0012_sync_applied_events.sql");
    const SCHEMA_V13: &str =
        include_str!("../../../src-tauri/src/db/sql/0013_sync_conflicts.sql");
}
