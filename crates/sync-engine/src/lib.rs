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

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

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
/// Consumed by `SyncStore::record_conflict` (added with the applier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    Event,
    Task,
    TaskList,
    Calendar,
    ColorLabel,
    Section,
}
