//! Shared types and traits for Aperio's cross-device synchronisation
//! (DESIGN.md §19).
//!
//! ## The sync model in one paragraph
//!
//! Each device writes its mutations as JSON-Lines [`SyncEvent`]s into
//! `sync/log/<timestamp>_<device_id>.jsonl`. Devices append-only —
//! nobody ever rewrites a log file. Periodically, devices fetch new
//! log files via a [`SyncAdapter`] (WebDAV, SFTP, local FS, …),
//! replay the events onto their local SQLite cache, and push their
//! own staged log files. A [`Snapshot`] is generated when the log
//! count / size / age cross thresholds; it folds older log files
//! into one JSON state dump so new devices have a cheap onboarding
//! ramp.
//!
//! ## What lives in this crate
//!
//! - [`SyncEvent`] — the wire-format enum every mutation flows
//!   through. One variant per spec-listed event type from §19.2.
//! - [`SyncAdapter`] — the storage-backend trait. WebDAV, SFTP,
//!   Dropbox, Google Drive, local filesystem all implement this.
//! - [`LogFile`] / [`Snapshot`] / [`MetaJson`] — the on-disk shapes
//!   the storage backend transports.
//! - [`DeviceId`] — UUID v4 generated on first run, persisted via
//!   `user_prefs`. Identifies the device in `meta.json.devices` and
//!   in log-file names.
//! - [`SyncError`] / [`SyncResult`] — the crate's error type.
//!
//! ## What this crate explicitly does NOT do
//!
//! - Writing events into the local DB: that's the event-log applier
//!   in `src-tauri/src/event_log.rs` (Phase Sc).
//! - Materialising events from local mutations: the writer hooks in
//!   the LocalAdapter / user_prefs / shortcut layer (Phase Sb).
//! - Implementing any particular adapter: every `sync-adapter-*`
//!   crate consumes this trait surface and writes its own wire code.
//! - Snapshot compaction: that's its own phase (Sg) and lives in the
//!   command layer where it has cross-table read access.

pub mod adapter;
pub mod device;
pub mod error;
pub mod event;
pub mod log;
pub mod meta;
pub mod snapshot;

pub use adapter::{DeviceCursor, SyncAdapter};
pub use device::{DeviceId, DEVICE_ID_PREF_KEY};
pub use error::{SyncError, SyncResult};
pub use event::{
    EventEnvelope, EventId, EventPayload, IdPayload, PartialPayload,
    PluginPayload, SettingsPayload, ShortcutKeyPayload, ShortcutPayload,
    SyncEvent,
};
pub use log::{LogFile, LogFileName};
pub use meta::{DeviceRecord, MetaJson, SCHEMA_VERSION};
pub use snapshot::{Snapshot, SnapshotMetadata};
