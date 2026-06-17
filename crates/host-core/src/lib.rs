//! Aperio host-core — the Tauri-free half of the desktop backend.
//!
//! This crate holds the host logic that has no business knowing about
//! Tauri: the SQLite handle, portable data-path resolution, account +
//! secret persistence, the adapter registry, and the cache / sync
//! stores. The desktop `src-tauri` backend and the mobile UniFFI host
//! (`cal-ffi`) both build on top of it, so the two run literally the
//! same engine (DESIGN.md section 22).
//!
//! Modules are extracted from `src-tauri` one dependency-ordered layer
//! at a time, each a behaviour-preserving move. `src-tauri` re-exports
//! what it moved out so existing `crate::<mod>` references keep
//! resolving.

pub mod accounts;
pub mod cache;
pub mod conflicts;
pub mod credential_sync;
pub mod db;
pub mod device_names;
pub mod paths;
pub mod registry;
pub mod remote_plugins;
pub mod sync_log;
pub mod user_prefs;

pub use db::{DbError, DbHandle, DbResult, SharedConn, CURRENT_SCHEMA_VERSION};
pub use paths::{resolve_data_dir, DataDirKind, DataDirResolution};
