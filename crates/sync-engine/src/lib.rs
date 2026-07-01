//! `sync-engine` — Aperio's platform-agnostic synchronisation engine.
//!
//! The cross-device sync model lives in [`sync_core`] (the on-disk shape,
//! the `SyncAdapter` storage-backend trait, event log, snapshot, meta).
//! This crate holds the *orchestration* of that model — the writer, applier,
//! orchestrator, compactor and snapshot builder — extracted out of the Tauri
//! desktop app so the **same engine** can run on the desktop and on mobile
//! (via UniFFI).
//!
//! The engine reaches the platform only through the traits below. Desktop and
//! mobile each provide their own implementations:
//!
//! - [`SyncStore`] — the local SQLite store + sync metadata *(added with the
//!   applier; see DESIGN-sync-engine.md §3.1)*.
//! - [`SyncBlobStore`] — the local sync working files (pending logs, sound
//!   assets).
//! - [`SecretStore`] — the platform credential store (keychain / keystore).
//! - [`Clock`] — injectable wall-clock time.
//! - [`SyncProgressReporter`] — status / conflict / log callbacks (replaces the
//!   desktop's direct `app.emit`).
//!
//! This is built up incrementally, desktop-first, keeping the desktop test
//! suite green at every step (DESIGN-sync-engine.md §5).

use std::collections::BTreeMap;

use cal_adapter_local::{SnapshotApplyReport, SnapshotDump};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod applier;
mod compactor;
mod orchestrator;
mod snapshot;
mod writer;
pub use applier::{conflict_still_genuine, ApplyReport, EventLogApplier};
pub use compactor::{
    CompactionReport, Compactor, DEFAULT_MAX_AGE_DAYS, DEFAULT_MAX_BYTES, DEFAULT_MAX_LOGS,
    PREF_BYTES_SINCE_SNAPSHOT, PREF_LOGS_SINCE_SNAPSHOT, PREF_MAX_AGE_DAYS, PREF_MAX_BYTES,
    PREF_MAX_LOGS,
};
pub use orchestrator::{
    SyncOrchestrator, SyncRoundHooks, SYNC_CURSOR_PREF_KEY, SYNC_LAST_ROUND_PREF_KEY,
    SYNC_OWN_NEWEST_LOG_PREF_KEY,
};
pub use snapshot::{
    AperioSnapshotBody, SnapshotAccount, SnapshotApplyOutcome, SnapshotBuilder, SnapshotCredential,
};
pub use writer::EventLogWriter;

pub mod whitelist;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

/// `user_prefs` key holding this device's human-readable name (surfaced
/// in `meta.json`'s device registry). Defined here — the canonical
/// engine-level sync pref — and re-exported by the desktop onboarding
/// module, which owns the UI that sets it.
pub const PREF_DEVICE_NAME: &str = "sync.deviceName";

/// `user_prefs` key holding the configured periodic sync interval, in
/// minutes. The orchestrator reads it for `SyncStatus`; the desktop
/// scheduler reads it to time its loop. Canonical definition here,
/// re-exported by the desktop scheduler.
pub const PREF_SYNC_INTERVAL_MINUTES: &str = "sync.intervalMinutes";

/// Default periodic sync interval (minutes) when the pref is unset.
pub const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 5;

// ─────────────────────────── round report / status ─────────────────────────
// Moved verbatim from src-tauri/src/event_log/orchestrator.rs so both the
// desktop app and the engine share one definition.

/// Result of one `sync_now()` invocation. Surfaced to the UI so the user can
/// see "12 events applied" or "no new changes".
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct SyncRoundReport {
    /// Pending log files successfully pushed to the remote.
    pub pushed_logs: usize,
    /// Log files pulled from the remote (after cursor + own-device filtering).
    pub fetched_logs: usize,
    /// Aggregate apply count across every fetched log.
    pub applied: usize,
    pub skipped_own: usize,
    pub skipped_already_applied: usize,
    pub skipped_unsupported: usize,
    /// Per-envelope apply failures (non-fatal; the round still succeeds).
    pub apply_failures: usize,
    /// Push failures logged but tolerated (one bad file shouldn't sink a round).
    pub push_failures: usize,
    /// Field-level conflicts recorded this round (DESIGN.md §19.3).
    pub conflicts: usize,
}

/// Read-only snapshot of the engine's state, surfaced by `get_sync_status`.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub configured: bool,
    pub in_flight: bool,
    pub last_synced_at: Option<String>,
    pub interval_minutes: u32,
    pub e2e_enabled: bool,
    pub schema_too_old: bool,
    pub min_app_version_required: Option<String>,
    /// Decorated by the scheduler (the engine defaults it to `false`).
    #[serde(default)]
    pub sustained_failure: bool,
    pub stale_device_since: Option<String>,
    /// Decorated by the scheduler; matches `sync_core::SyncError::code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
}

// ─────────────────────────────────── secrets ───────────────────────────────

/// One logical slot per account (DESIGN.md §6.6). The on-disk service name is
/// the platform impl's concern; this enum + the sync allowlist are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSlot {
    /// Short-lived OAuth2 access token.
    AccessToken,
    /// OAuth2 refresh token (long-lived).
    RefreshToken,
    /// Generic password (Basic Auth: CalDAV, WebDAV, …).
    Password,
    /// API token (Vikunja, Todoist, …).
    ApiToken,
    /// 32-byte AES-256 key for cross-device E2E (base64 in the store).
    SyncEncryptionKey,
}

impl SecretSlot {
    /// Public wire name — used as the keychain service suffix and as the slot
    /// name in `credential.set` events.
    pub fn wire_name(self) -> &'static str {
        match self {
            SecretSlot::AccessToken => "access_token",
            SecretSlot::RefreshToken => "refresh_token",
            SecretSlot::Password => "password",
            SecretSlot::ApiToken => "api_token",
            SecretSlot::SyncEncryptionKey => "sync_encryption_key",
        }
    }

    /// Map a wire slot name back to a slot, but ONLY for slots allowed to
    /// travel through cross-device credential sync. The short-lived access
    /// token (re-derived per device) and the E2E key itself (syncing it would
    /// defeat E2E) are deliberately rejected — the single allowlist that
    /// decides what a received credential event may write.
    pub fn syncable_from_wire(name: &str) -> Option<SecretSlot> {
        match name {
            "password" => Some(SecretSlot::Password),
            "refresh_token" => Some(SecretSlot::RefreshToken),
            "api_token" => Some(SecretSlot::ApiToken),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("no secret stored for this account/slot")]
    NotFound,
    #[error("keychain error: {0}")]
    Backend(String),
}

/// Platform credential store (keychain / keystore). Desktop: the `keyring`
/// crate; mobile: iOS Keychain / Android Keystore over the FFI boundary.
pub trait SecretStore: Send + Sync {
    /// Persist `value` for `(account_id, slot)`, overwriting any prior value.
    fn store(&self, account_id: &str, slot: SecretSlot, value: &str) -> Result<(), SecretError>;
    /// Read the value for `(account_id, slot)`; `NotFound` when absent.
    fn retrieve(&self, account_id: &str, slot: SecretSlot) -> Result<String, SecretError>;
    /// Best-effort removal; a missing entry is `Ok(())`.
    fn delete(&self, account_id: &str, slot: SecretSlot) -> Result<(), SecretError>;
    /// Clear every slot tied to `account_id`.
    fn delete_all(&self, account_id: &str) -> Result<(), SecretError>;
}

// ──────────────────────────────────── store ────────────────────────────────

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("store error: {0}")]
    Backend(String),
}

/// The local store seam used by the snapshot builder (and, later, the
/// applier / compactor / orchestrator): the calendar/event/task rows,
/// the whitelisted `user_prefs` settings, the non-secret account rows,
/// and the cross-device E2E flag.
///
/// Desktop implements this over its SQLite (`LocalAdapter` for the rows,
/// `UserPrefsRepo` + ad-hoc SQL for settings/accounts); mobile over the
/// same `LocalAdapter` plus its own settings/account storage. Secrets are
/// a separate seam ([`SecretStore`]) because they live in the platform
/// keychain, never in the SQLite store.
pub trait SyncStore: Send + Sync {
    /// Dump every local calendar/event/task/list/label row for a
    /// snapshot body, preserving wire ids (so `apply_snapshot_dump`
    /// round-trips them).
    fn dump_for_snapshot(&self) -> Result<SnapshotDump, StoreError>;
    /// Restore a snapshot's rows; the returned report carries the
    /// per-section applied/failed counts.
    fn apply_snapshot_dump(&self, dump: &SnapshotDump) -> Result<SnapshotApplyReport, StoreError>;
    /// The whitelisted `user_prefs` rows (key → value). Only keys that
    /// pass [`whitelist::is_synced_key`] are returned.
    fn dump_synced_settings(&self) -> Result<BTreeMap<String, String>, StoreError>;
    /// Read one `user_prefs` row, `None` when absent. Used for the
    /// engine's own device-local state — compaction counters and
    /// thresholds, the device name, the sync cursor.
    fn get_pref(&self, key: &str) -> Result<Option<String>, StoreError>;
    /// Write one `user_prefs` row WITHOUT emitting an event-log event
    /// (the snapshot apply + compaction paths must not loop back through
    /// the writer). Whether a key is a *synced* setting is the caller's
    /// policy (the whitelist gate), not this primitive's.
    fn set_pref(&self, key: &str, value: &str) -> Result<(), StoreError>;
    /// Every external account row (excludes the implicit `local` one).
    fn dump_accounts(&self) -> Result<Vec<SnapshotAccount>, StoreError>;
    /// Insert-or-update one account row from a snapshot (skips `local`).
    fn upsert_account(&self, account: &SnapshotAccount) -> Result<(), StoreError>;
    /// Whether cross-device end-to-end encryption is enabled on this
    /// device — gates whether account secrets ride along in the body.
    fn e2e_enabled(&self) -> bool;

    // --- applier seam (DESIGN-sync-engine.md §3.1) -----------------------

    /// Has the event with this id already been integrated? Backed by the
    /// `sync_applied_events` idempotency table — re-fetches and
    /// overlapping log files both resolve to `true` here and are skipped.
    fn is_event_applied(&self, event_id: &str) -> Result<bool, StoreError>;
    /// Record that the event with this id has been integrated, so a later
    /// pass skips it. Idempotent (INSERT OR IGNORE).
    fn mark_event_applied(&self, event_id: &str) -> Result<(), StoreError>;
    /// Persist a field-level conflict the applier couldn't auto-merge,
    /// superseding any prior unresolved conflict on the same
    /// (kind, row, field) (DESIGN.md §19.3).
    fn record_conflict(&self, conflict: &NewConflict) -> Result<(), StoreError>;
    /// Delete one `user_prefs` row (the `settings.updated` apply path
    /// encodes a JSON-null value as "remove the key").
    fn delete_pref(&self, key: &str) -> Result<(), StoreError>;
    /// Remove one external account row (an `account.deleted` event from
    /// another device). The implicit `local` account is never touched.
    fn delete_account(&self, id: &str) -> Result<(), StoreError>;
    /// Mirror a remote plugin announcement (`plugin.installed` /
    /// `plugin.updated`) into the local store so the Settings → Plugins
    /// panel can surface "needed plugins" (DESIGN.md §20.8).
    fn upsert_remote_plugin(
        &self,
        id: &str,
        name: Option<&str>,
        version: &str,
        plugin_type: Option<&str>,
        source: Option<&str>,
        announced_by_device: &str,
    ) -> Result<(), StoreError>;
    /// Drop a remote plugin announcement (`plugin.uninstalled`).
    fn delete_remote_plugin(&self, id: &str) -> Result<(), StoreError>;
}

// ──────────────────────────────────── clock ────────────────────────────────

/// Injectable wall-clock. Real impls return `Utc::now()`; tests can freeze it.
/// Needed for the session-file-timestamp / `boot_at` invariants
/// (DESIGN-sync-engine.md §4).
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

// ────────────────────────────────── blob store ─────────────────────────────

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("blob store error: {0}")]
    Backend(String),
}

/// One entry returned by [`SyncBlobStore::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobEntry {
    /// Logical path relative to the blob root (e.g. `pending/<ts>_<dev>.jsonl`).
    pub path: String,
    pub size: u64,
}

/// The local sync working files: the pending event logs and the sound assets
/// (the *remote* logs/snapshot/meta belong to the [`sync_core::SyncAdapter`]).
/// Desktop: `tokio::fs` over the data dir; mobile: the app sandbox.
#[async_trait::async_trait]
pub trait SyncBlobStore: Send + Sync {
    /// Write `data` at `path`, creating parent directories as needed.
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), BlobError>;
    /// Read the bytes at `path`.
    async fn read(&self, path: &str) -> Result<Vec<u8>, BlobError>;
    /// List entries under `prefix`.
    async fn list(&self, prefix: &str) -> Result<Vec<BlobEntry>, BlobError>;
    /// Delete a file (missing file is `Ok(())`).
    async fn delete(&self, path: &str) -> Result<(), BlobError>;
    /// Atomically rename (used by the writer's session rotation).
    async fn rename(&self, from: &str, to: &str) -> Result<(), BlobError>;
}

// ──────────────────────────────── progress reporter ────────────────────────

/// Status / conflict / log callbacks. Replaces the desktop's direct
/// `app.emit(...)`. Desktop: a Tauri emitter; mobile: an FFI callback to JS.
pub trait SyncProgressReporter: Send + Sync {
    /// Fired before and after each round; `report` is `Some` on completion.
    fn on_status_changed(&self, status: &SyncStatus, report: Option<&SyncRoundReport>);
    /// Fired when a round recorded new field-level conflicts.
    fn on_conflicts_detected(&self, count: usize);
    /// Fired when the sync-log audit trail changed (settings panel refresh).
    fn on_sync_log_updated(&self);
}

// ───────────────────────────────── conflicts ───────────────────────────────

/// Which row kind a field-level conflict belongs to (DESIGN.md §19.3).
/// One entry per synchronisable table that produces diff-style updates —
/// sections are full-row last-write-wins, so they never appear here.
/// Consumed by [`SyncStore::record_conflict`]. The desktop conflict
/// repository (and its `sync_conflicts` table) round-trips it via
/// [`ConflictKind::as_str`] / [`ConflictKind::from_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Event,
    Task,
    TaskList,
    Calendar,
    ColorLabel,
}

impl ConflictKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictKind::Event => "event",
            ConflictKind::Task => "task",
            ConflictKind::TaskList => "task_list",
            ConflictKind::Calendar => "calendar",
            ConflictKind::ColorLabel => "color_label",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "event" => Some(ConflictKind::Event),
            "task" => Some(ConflictKind::Task),
            "task_list" => Some(ConflictKind::TaskList),
            "calendar" => Some(ConflictKind::Calendar),
            "color_label" => Some(ConflictKind::ColorLabel),
            _ => None,
        }
    }
}

/// Insert-side shape for a field-level conflict — the values the applier
/// hands to [`SyncStore::record_conflict`] when it can't auto-merge a
/// field (DESIGN.md §19.3). Both value strings are JSON-encoded. The
/// stored/read shape (with `id`, `detected_at`, resolution columns) stays
/// desktop-side in the conflict repository.
#[derive(Debug, Clone)]
pub struct NewConflict {
    pub row_kind: ConflictKind,
    pub row_id: String,
    pub field: String,
    pub local_value: Option<String>,
    pub remote_value: Option<String>,
    pub remote_device_id: String,
    pub remote_timestamp: DateTime<Utc>,
}
