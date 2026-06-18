//! The mobile `Host` — the on-device counterpart to the desktop
//! `src-tauri` backend, assembled from the shared `host-core` crate.
//!
//! Where [`crate::LocalStore`] serves only the local SQLite task store,
//! `Host` owns the full account + adapter surface: it opens the same
//! migrated database, statically links + registers all 17 bundled
//! adapter plugins (no dlopen — iOS forbids it; see `host-plugins`),
//! and drives the same [`host_core::registry::AdapterRegistry`] the
//! desktop uses to route per-account reads/writes through external
//! services (CalDAV, EWS, …).
//!
//! ## Secret seam
//!
//! Credentials never touch the SQLite file. The desktop stores them in
//! the OS keyring; mobile implements the [`KeychainBridge`] foreign
//! trait over the iOS Keychain / Android Keystore. [`BridgeSecretStore`]
//! adapts that bridge to the engine-side [`sync_engine::SecretStore`]
//! the registry already routes through.
//!
//! ## Scope
//!
//! Account CRUD, the calendar/event surface, and cross-device sync. The async
//! `CalendarFeature` methods + the sync orchestrator are driven by a single
//! one-worker multi-thread tokio runtime via `block_on` (the worker keeps the
//! event-log writer's drain task advancing); account CRUD stays synchronous.
//! Every LOCAL mutation (calendars, events, accounts) is logged to the shared
//! event log so the next sync round carries it; account secrets also flow when
//! E2E is enabled (the credential-sync gate). `configure_sync_adapter_json` +
//! `sync_now_json` + `sync_status_json` + `push_now` make the Host a full sync
//! peer (the local-filesystem target this slice; webdav/sftp/ftp + OAuth kinds
//! follow).
//!
//! Deferred (documented per method): the pre-persist credential smoke-test; the
//! SWR read cache + cache-updated callback; colour resolution, overrides,
//! birthday calendars, cross-calendar event moves, free/busy + RSVP; the
//! SyncProgressBridge live-progress push callback + the E2E `wrap_if_encrypted`
//! branch; task/list/section sync (those live on
//! the separate `LocalStore`, which folds into this Host later). External
//! event paths are wired like local but hit the provider live (no cache),
//! exercised on-device, not in unit tests.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use cal_adapter_local::LocalAdapter;
use cal_core::{
    Calendar, CalendarFeature, ColorLabelId, ContactList, ContactsFeature, DateRange, Event,
    NewEvent, TaskList, TasksFeature,
};
use host_core::accounts::{AccountsRepo, AdapterKind};
use host_core::db::SharedConn;
use host_core::event_log::OnboardingService;
use host_core::overrides::{
    apply_color_to_calendars, apply_color_to_contact_lists, apply_color_to_events,
    apply_color_to_sections, apply_color_to_task_lists, apply_to_calendars, apply_to_task_lists,
    ContainerKind, OverridesError, OverridesRepo,
};
use host_core::registry::{AdapterRegistry, LOCAL_ID};
use host_core::sftp_host_keys::UserPrefsHostKeyVerifier;
use host_core::sync::build_orchestrator;
use host_core::user_prefs::UserPrefsRepo;
use host_core::DbHandle;
use plugin_core::manifest::TaskCapabilities;
use plugin_core::shim::FfiSyncAdapter;
use plugin_core::PluginManager;
use sync_core::{
    derive_key, fresh_data_key, resolve_data_key, wrap_key, AccountPayload, EncryptingAdapter,
    EncryptionParams, EventPayload, IdPayload, SettingsPayload, SyncAdapter, SyncError, SyncEvent,
    KEY_LEN,
};
use sync_engine::{EventLogWriter, SecretError, SecretSlot, SecretStore, SyncOrchestrator};

/// Sync-adapter pref keys (device-local; never propagated). Match the desktop
/// `commands::sync` keys so the same SQLite row layout serves both backends.
const PREF_ADAPTER_KIND: &str = "sync.adapter.kind";
const PREF_LOCAL_PATH: &str = "sync.adapter.local.path";
/// WebDAV adapter config keys. The URL + user are device-local prefs (never
/// propagated); the password lives in the keychain under a fixed pseudo-account
/// so the adapter owns one managed slot independent of any user account row.
const PREF_WEBDAV_URL: &str = "sync.adapter.webdav.url";
const PREF_WEBDAV_USER: &str = "sync.adapter.webdav.user";
const WEBDAV_SECRET_ACCOUNT: &str = "sync.adapter.webdav";
/// FTPS adapter config keys. Same device-local / never-synced guarantee as the
/// WebDAV pair; the password lives in the keychain under FTP_SECRET_ACCOUNT.
const PREF_FTP_HOST: &str = "sync.adapter.ftp.host";
const PREF_FTP_PORT: &str = "sync.adapter.ftp.port";
const PREF_FTP_USER: &str = "sync.adapter.ftp.user";
const PREF_FTP_PATH: &str = "sync.adapter.ftp.path";
const PREF_FTP_MODE: &str = "sync.adapter.ftp.mode";
const FTP_SECRET_ACCOUNT: &str = "sync.adapter.ftp";
/// Dropbox / Google Drive OAuth sync-adapter keys. client_id (+ Google's
/// secret) + the dataset path/folder are device-local prefs; the refresh token
/// lives in the keychain under a fixed pseudo-account (one managed slot per
/// adapter kind, divorced from any user account row — same keying as desktop).
const PREF_DROPBOX_CLIENT_ID: &str = "sync.adapter.dropbox.clientId";
const PREF_DROPBOX_CLIENT_SECRET: &str = "sync.adapter.dropbox.clientSecret";
const PREF_DROPBOX_PATH: &str = "sync.adapter.dropbox.path";
const DROPBOX_SECRET_ACCOUNT: &str = "sync.adapter.dropbox";
const PREF_GOOGLEDRIVE_CLIENT_ID: &str = "sync.adapter.googledrive.clientId";
const PREF_GOOGLEDRIVE_CLIENT_SECRET: &str = "sync.adapter.googledrive.clientSecret";
const PREF_GOOGLEDRIVE_FOLDER_NAME: &str = "sync.adapter.googledrive.folderName";
const GOOGLEDRIVE_SECRET_ACCOUNT: &str = "sync.adapter.googledrive";
/// SFTP adapter keys. host/port/user/path/auth-method/key-path are device-local
/// prefs; the password (or SSH-key passphrase) lives in the keychain under one
/// of two pseudo-accounts so switching auth methods doesn't clobber the other
/// family's stored credential. The trusted host-key fingerprint is NOT here —
/// it's in the shared host-core `UserPrefsHostKeyVerifier` (user_prefs
/// `sync.adapter.sftp.knownHosts.<host:port>`, device-local, §19.5 TOFU).
const PREF_SFTP_HOST: &str = "sync.adapter.sftp.host";
const PREF_SFTP_PORT: &str = "sync.adapter.sftp.port";
const PREF_SFTP_USER: &str = "sync.adapter.sftp.user";
const PREF_SFTP_PATH: &str = "sync.adapter.sftp.path";
const PREF_SFTP_AUTH_METHOD: &str = "sync.adapter.sftp.authMethod";
const PREF_SFTP_KEY_PATH: &str = "sync.adapter.sftp.keyPath";
const SFTP_SECRET_ACCOUNT: &str = "sync.adapter.sftp";
const SFTP_KEY_SECRET_ACCOUNT: &str = "sync.adapter.sftp.key";
/// Plugin ids of the statically-embedded sync adapters this host configures.
const PLUGIN_ID_SYNC_LOCAL: &str = "com.aperio.sync-adapter-local";
const PLUGIN_ID_WEBDAV: &str = "com.aperio.sync-adapter-webdav";
const PLUGIN_ID_FTP: &str = "com.aperio.sync-adapter-ftp";
const PLUGIN_ID_DROPBOX: &str = "com.aperio.sync-adapter-dropbox";
const PLUGIN_ID_GOOGLEDRIVE: &str = "com.aperio.sync-adapter-googledrive";
const PLUGIN_ID_SFTP: &str = "com.aperio.sync-adapter-sftp";
/// Fixed keychain pseudo-account holding the E2E data key (base64) under
/// `SecretSlot::SyncEncryptionKey`. Device-local by design — the key is NEVER
/// synced (syncing it would defeat E2E), so a joining device derives it from a
/// passphrase via the onboarding flow. Matches the desktop's `E2E_SECRET_ACCOUNT`.
const E2E_SECRET_ACCOUNT: &str = "sync.adapter.e2e";

fn sync_err(e: SyncError) -> StoreError {
    StoreError::Storage {
        detail: e.to_string(),
    }
}

/// Map any `Display` write error (user_prefs set, keychain store) raised while
/// persisting a sync-adapter config into the generic storage error.
fn storage_err(e: impl std::fmt::Display) -> StoreError {
    StoreError::Storage {
        detail: e.to_string(),
    }
}

// ── E2E sync encryption (§19.7) ──────────────────────────────────────────────
//
// The data key (DEK) is a 32-byte AES-256 key kept device-local in the keychain
// (base64 under E2E_SECRET_ACCOUNT / SyncEncryptionKey) — NEVER synced. A
// SyncAdapter is wrapped in `EncryptingAdapter` when the dataset is E2E. The
// crypto + the onboarding key-derivation are the shared sync-core/host-core code
// the desktop uses (faithful reuse, no bespoke crypto here).

/// Read the device-local E2E data key from the keychain (base64-decoded), or
/// `None` when absent / malformed.
fn load_e2e_key(secret_store: &dyn SecretStore) -> Option<[u8; KEY_LEN]> {
    let raw = secret_store
        .retrieve(E2E_SECRET_ACCOUNT, SecretSlot::SyncEncryptionKey)
        .ok()?;
    let bytes = BASE64.decode(raw.trim()).ok()?;
    if bytes.len() != KEY_LEN {
        return None;
    }
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Persist the E2E data key (base64) in the device-local keychain slot.
fn store_e2e_key(secret_store: &dyn SecretStore, key: &[u8; KEY_LEN]) -> Result<(), StoreError> {
    secret_store
        .store(
            E2E_SECRET_ACCOUNT,
            SecretSlot::SyncEncryptionKey,
            &BASE64.encode(key),
        )
        .map_err(|e| StoreError::Storage {
            detail: format!("store E2E key: {e}"),
        })
}

/// Drop the device-local E2E data key from the keychain (best-effort) — used by
/// the disable-encryption downgrade. Mirrors the desktop `delete_e2e_key`.
fn delete_e2e_key(secret_store: &dyn SecretStore) {
    let _ = secret_store.delete(E2E_SECRET_ACCOUNT, SecretSlot::SyncEncryptionKey);
}

/// Wrap `plain` in an `EncryptingAdapter` when a key is present; otherwise pass
/// it through untouched. Mirrors the desktop `wrap_if_encrypted`.
fn wrap_if_encrypted(
    plain: Arc<dyn SyncAdapter>,
    key: Option<[u8; KEY_LEN]>,
) -> Arc<dyn SyncAdapter> {
    match key {
        Some(k) => Arc::new(EncryptingAdapter::new(plain, k)),
        None => plain,
    }
}

/// Error for an external task list operation that mobile keeps local-only.
/// Task + section CONTENT writes (create/update/delete task, create/update/
/// delete section) DO route to the provider; only re-parenting an external list
/// stays unsupported (nesting is provider-specific + needs move semantics the
/// mobile signature doesn't carry). A clear `Unsupported` beats a confusing
/// local NotFound.
fn external_reparent_unsupported() -> StoreError {
    StoreError::Unsupported {
        detail: "reparenting a task list from an external account is not supported on mobile yet"
            .to_string(),
    }
}

/// Map an `OverridesRepo` failure into the bridge's `StoreError` (the generic
/// `map_store_err` only covers `cal_core::Error`). An empty name is the one
/// user-facing validation; everything else is a SQLite-layer failure.
fn map_overrides_err(e: OverridesError) -> StoreError {
    match e {
        OverridesError::EmptyName => StoreError::InvalidField {
            field: "name".to_string(),
            detail: "name must not be empty".to_string(),
        },
        OverridesError::Sqlite(err) => StoreError::Storage {
            detail: err.to_string(),
        },
    }
}

/// Parse the FFI `kind` string into a `ContainerKind`, or a clear
/// `InvalidField` for an unknown value (the FFI can't cheaply share the enum).
fn parse_container_kind(kind: &str) -> Result<ContainerKind, StoreError> {
    ContainerKind::parse(kind).ok_or_else(|| StoreError::InvalidField {
        field: "kind".to_string(),
        detail: format!("unknown container kind '{kind}'"),
    })
}

/// Consecutive-failure count at which sync is reported as `sustained_failure`.
/// Matches the desktop `SyncScheduler` latch threshold.
const SUSTAINED_FAILURE_THRESHOLD: u32 = 3;

/// The mobile failure latch. The orchestrator's `SyncStatus` always reports
/// `sustained_failure: false` / `last_error_code: None` — on the desktop the
/// `SyncScheduler` decorates those across rounds, but mobile has no scheduler.
/// This tiny driver does the same job: `sync_now`/`push_now` record each round's
/// outcome and `sync_status_json` reads the latch to decorate the status, so a
/// blind user learns when sync has been failing repeatedly (not just once).
#[derive(Default)]
struct SyncProgressDriver {
    consecutive_failures: AtomicU32,
    last_error_code: Mutex<Option<String>>,
}

impl SyncProgressDriver {
    /// A clean round resets the latch.
    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        *self.last_error_code.lock().expect("latch poison") = None;
    }

    /// A failed round bumps the streak and latches the stable error code.
    fn record_failure(&self, code: &str) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        *self.last_error_code.lock().expect("latch poison") = Some(code.to_string());
    }

    /// Whether the failure streak has reached the sustained threshold.
    fn sustained(&self) -> bool {
        self.consecutive_failures.load(Ordering::Relaxed) >= SUSTAINED_FAILURE_THRESHOLD
    }

    /// The most recently latched error code (cleared on success).
    fn last_code(&self) -> Option<String> {
        self.last_error_code.lock().expect("latch poison").clone()
    }
}

/// Build the non-secret `AccountPayload` an `account.*` sync event carries
/// (mirrors the desktop `account_payload`). Secrets never travel here — they go
/// through the credential-sync gate (E2E only).
fn account_payload(acc: &host_core::accounts::Account) -> AccountPayload {
    AccountPayload {
        id: acc.id.clone(),
        adapter_kind: acc.adapter_kind.as_str().to_string(),
        display_name: acc.display_name.clone(),
        config_json: acc.config_json.clone(),
        created_at: acc.created_at.clone(),
        updated_at: acc.updated_at.clone(),
    }
}

use crate::{from_json, map_store_err, to_json, StoreError};

/// Errors the foreign keychain implementation can raise. Mirrors
/// [`sync_engine::SecretError`] so the `NotFound` distinction the
/// registry branches on (e.g. an absent optional iCal password) survives
/// the round-trip.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KeychainError {
    /// No secret stored for this `(account, slot)`.
    #[error("secret not found")]
    NotFound,
    /// The platform keychain/keystore backend failed.
    #[error("keychain backend error: {detail}")]
    Backend { detail: String },
}

/// Platform credential store, implemented on the foreign side (iOS
/// Keychain / Android Keystore). `slot` is the stable wire name from
/// [`SecretSlot::wire_name`] (`"password"`, `"access_token"`, …) so the
/// foreign code can key its store without depending on the Rust enum.
#[uniffi::export(with_foreign)]
pub trait KeychainBridge: Send + Sync {
    /// Persist `value` for `(account_id, slot)`, overwriting any prior.
    fn store(&self, account_id: String, slot: String, value: String) -> Result<(), KeychainError>;
    /// Read the value for `(account_id, slot)`; `NotFound` when absent.
    fn retrieve(&self, account_id: String, slot: String) -> Result<String, KeychainError>;
    /// Best-effort removal; a missing entry is `Ok(())`.
    fn delete(&self, account_id: String, slot: String) -> Result<(), KeychainError>;
    /// Clear every slot tied to `account_id`.
    fn delete_all(&self, account_id: String) -> Result<(), KeychainError>;
}

/// Adapts a foreign [`KeychainBridge`] to the engine-side
/// [`SecretStore`] the [`AdapterRegistry`] routes credentials through.
struct BridgeSecretStore {
    bridge: Arc<dyn KeychainBridge>,
}

fn to_secret_err(e: KeychainError) -> SecretError {
    match e {
        KeychainError::NotFound => SecretError::NotFound,
        KeychainError::Backend { detail } => SecretError::Backend(detail),
    }
}

impl SecretStore for BridgeSecretStore {
    fn store(&self, account_id: &str, slot: SecretSlot, value: &str) -> Result<(), SecretError> {
        self.bridge
            .store(
                account_id.to_string(),
                slot.wire_name().to_string(),
                value.to_string(),
            )
            .map_err(to_secret_err)
    }

    fn retrieve(&self, account_id: &str, slot: SecretSlot) -> Result<String, SecretError> {
        self.bridge
            .retrieve(account_id.to_string(), slot.wire_name().to_string())
            .map_err(to_secret_err)
    }

    fn delete(&self, account_id: &str, slot: SecretSlot) -> Result<(), SecretError> {
        self.bridge
            .delete(account_id.to_string(), slot.wire_name().to_string())
            .map_err(to_secret_err)
    }

    fn delete_all(&self, account_id: &str) -> Result<(), SecretError> {
        self.bridge
            .delete_all(account_id.to_string())
            .map_err(to_secret_err)
    }
}

/// Account-creation request from the foreign side. Same wire shape as the
/// desktop's `CreateAccountRequest` (snake-case `adapter_kind`), so the
/// shared TS domain logic emits one payload for both backends.
#[derive(serde::Deserialize)]
struct NewAccountRequest {
    adapter_kind: AdapterKind,
    display_name: String,
    #[serde(default = "default_config_json")]
    config_json: String,
    /// Secret half of the credentials (CalDAV password, API token, …).
    /// Stored only via the keychain bridge, never in the SQLite file.
    #[serde(default)]
    secret: Option<String>,
}

fn default_config_json() -> String {
    "{}".into()
}

/// A calendar enriched with the owning `account_id`, matching the desktop
/// `CalendarRow` wire shape the frontend groups by source. `Calendar`'s fields
/// are flattened to the top level (so the JSON is `{id, name, …, account_id}`,
/// not `{inner: {...}, account_id}`).
///
/// `recurrence_capabilities` is intentionally omitted in this slice (the TS
/// field is optional and the frontend defaults to full RFC-5545 support — the
/// same default the desktop yields for the local / unknown account); it lands
/// with the plugin-manifest capability port.
#[derive(serde::Serialize)]
struct CalendarRow {
    #[serde(flatten)]
    inner: Calendar,
    account_id: String,
}

impl CalendarRow {
    fn new(inner: Calendar, account_id: String) -> Self {
        Self { inner, account_id }
    }
}

/// Create-calendar request (the desktop `CreateCalendarRequest` shape, minus
/// the not-yet-wired colour object). Local calendars only.
#[derive(serde::Deserialize)]
struct CreateCalendarRequest {
    name: String,
    #[serde(default)]
    color_label: Option<String>,
}

/// Event-range read request — the desktop `get_events` payload. `start`/`end`
/// are RFC-3339 UTC instants (chrono parses them).
#[derive(serde::Deserialize)]
struct EventRangeRequest {
    calendar_id: String,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
}

/// Create-event request — the target calendar plus a flattened `NewEvent`
/// (the desktop `create_event` payload shape).
#[derive(serde::Deserialize)]
struct CreateEventRequest {
    calendar_id: String,
    #[serde(flatten)]
    event: NewEvent,
}

/// Configure-sync request — a flattened mirror of the desktop `SyncAdapterConfig`
/// enum. Each kind reads its own subset of the optional fields:
///   - `local`  → `path` (filesystem path).
///   - `webdav` → `url` + `user` + `password` (password optional; omit/empty
///                reuses the stored keychain secret).
///   - `ftp`    → `host` + `port` + `user` + `path` + `mode` + `password`
///                (same password-reuse contract).
///   - `dropbox` / `googledrive` → `client_id` (+ Google's `client_secret`) +
///                the dataset `path`/`folder_name`; the refresh token must
///                already be in the keychain (run `complete_sync_oauth_json`
///                after the native auth session first).
/// SFTP (host-key trust flow) follows in its own phase.
#[derive(serde::Deserialize)]
struct ConfigureSyncRequest {
    kind: String,
    /// `local` filesystem path / `ftp` remote path / `dropbox` base path.
    #[serde(default)]
    path: Option<String>,
    /// `webdav` collection URL.
    #[serde(default)]
    url: Option<String>,
    /// `webdav` / `ftp` username.
    #[serde(default)]
    user: Option<String>,
    /// `webdav` / `ftp` password. Omitted or empty reuses the keychain secret
    /// so URL/host edits don't require re-typing.
    #[serde(default)]
    password: Option<String>,
    /// `ftp` host.
    #[serde(default)]
    host: Option<String>,
    /// `ftp` port (defaults to 21 — explicit FTPS).
    #[serde(default)]
    port: Option<u16>,
    /// `ftp` TLS mode: `"explicit"` (default), `"implicit"`, or `"plain"`.
    #[serde(default)]
    mode: Option<String>,
    /// `dropbox` / `googledrive` OAuth client id.
    #[serde(default)]
    client_id: Option<String>,
    /// `dropbox` (optional, PKCE public app) / `googledrive` (required) secret.
    #[serde(default)]
    client_secret: Option<String>,
    /// `googledrive` dataset folder name.
    #[serde(default)]
    folder_name: Option<String>,
    /// `sftp` auth method: `"password"` (default) or `"key"`.
    #[serde(default)]
    auth_method: Option<String>,
    /// `sftp` key-auth: path to the private key file (on the device).
    #[serde(default)]
    key_path: Option<String>,
    /// `sftp` key-auth: the key's passphrase. Omitted/empty reuses the stored
    /// keychain secret (same contract as `password`).
    #[serde(default)]
    key_passphrase: Option<String>,
}

/// The mobile app's handle to the full on-device engine.
#[derive(uniffi::Object)]
pub struct Host {
    db: DbHandle,
    /// The local calendar/task adapter — the routing branch for the implicit
    /// `local` account, sharing the one writer connection with the registry's
    /// external adapters (the desktop topology).
    adapter: LocalAdapter,
    registry: Arc<AdapterRegistry>,
    secret_store: Arc<dyn SecretStore>,
    /// Drives the async `CalendarFeature` methods + the sync orchestrator via
    /// `block_on`. ONE worker thread (multi-thread flavour, not current-thread)
    /// so the event-log writer's long-lived drain task keeps advancing between
    /// our `block_on` calls — a current-thread runtime would only drive it
    /// while we're parked in `block_on`. Exported methods stay synchronous and
    /// wrap their async work in exactly one `self.runtime.block_on(..)`.
    runtime: tokio::runtime::Runtime,
    /// Appends a `SyncEvent` after each LOCAL mutation so the next sync round
    /// carries it. External-account mutations self-sync via the provider, so
    /// they are NOT logged here (mirrors the desktop `is_local` split).
    writer: Arc<EventLogWriter>,
    /// Runs a sync round (push + fetch + apply). Unconfigured until
    /// `configure_sync_adapter_json`; `sync_now` then errors "not configured".
    orchestrator: Arc<SyncOrchestrator>,
    /// The onboarding engine (snapshot consume/produce + meta heartbeats) —
    /// drives the E2E establishment (`enable_sync_encryption_json` adopts a fresh
    /// encrypted dataset; join-existing is a later phase).
    onboarding: Arc<OnboardingService>,
    /// The static plugin registry — the registry holds an `Arc` clone for
    /// per-account adapters; the Host also opens the sync-adapter plugin here.
    plugin_manager: Arc<PluginManager>,
    /// Failure latch for `sustained_failure` / `last_error_code` in the sync
    /// status (the desktop scheduler's job; mobile has no scheduler).
    progress: SyncProgressDriver,
}

impl Host {
    /// Resolve a calendar id's owning adapter for the event methods: `None`
    /// is the local branch (the desktop's `.unwrap_or_else(LOCAL_ID)`
    /// fallback — an unknown id is treated as local), `Some(ext)` the external
    /// adapter. A non-local id whose adapter isn't live is `NotFound`,
    /// mirroring the desktop's "calendar is not routable" 404.
    ///
    /// Routing relies on the calendar→account map, which
    /// [`Host::list_calendars_json`] / [`Host::create_calendar_json`] prime —
    /// callers list calendars before event ops (the desktop invariant).
    fn route(&self, calendar_id: &str) -> Result<Option<Arc<dyn CalendarFeature>>, StoreError> {
        let account = self
            .registry
            .account_for_calendar(calendar_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        if account == LOCAL_ID {
            Ok(None)
        } else {
            self.registry
                .calendar_adapter(&account)
                .map(Some)
                .ok_or(StoreError::NotFound)
        }
    }

    /// Whether `calendar_id` belongs to the local account (an unknown id is
    /// treated as local, matching `route`). Drives the append-to-event-log
    /// decision: only LOCAL mutations are logged for sync; external accounts
    /// self-sync via their provider.
    fn is_local_calendar(&self, calendar_id: &str) -> bool {
        self.registry
            .account_for_calendar(calendar_id)
            .is_none_or(|a| a == LOCAL_ID)
    }

    /// E2E gate for a freshly-built sync adapter, run between `test_connection`
    /// and `orchestrator.configure` in every configure arm (mirrors the desktop
    /// `configure_sync_adapter`, src-tauri/src/commands/sync.rs). Inspect the
    /// target's `meta.json`: when it's an end-to-end-encrypted dataset, wrap the
    /// adapter in `EncryptingAdapter` with this device's data key. We do NOT
    /// re-derive the key here (that needs the passphrase) — if the target is
    /// encrypted but this device holds no key, we REFUSE rather than configure a
    /// plaintext adapter against an encrypted dataset (which would push readable
    /// logs into it and corrupt/leak the dataset). The passphrase-join flow is
    /// the only way to obtain the key for a foreign encrypted target. Keeps
    /// `PREF_E2E_ENABLED` in step with what the target actually is, so the next
    /// boot's restore wraps (or doesn't) to match.
    fn wrap_for_target(
        &self,
        adapter: Arc<dyn SyncAdapter>,
    ) -> Result<Arc<dyn SyncAdapter>, StoreError> {
        let meta = self
            .runtime
            .block_on(async { adapter.fetch_meta().await })
            .map_err(sync_err)?;
        let e2e_target = meta.as_ref().map(|m| m.e2e_enabled).unwrap_or(false);
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        if e2e_target {
            let key = load_e2e_key(self.secret_store.as_ref()).ok_or_else(|| StoreError::Auth {
                detail: "this sync target is end-to-end encrypted; join it with the \
                         passphrase on this device before configuring it"
                    .to_string(),
            })?;
            prefs
                .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
                .map_err(storage_err)?;
            Ok(wrap_if_encrypted(adapter, Some(key)))
        } else {
            // Plaintext target: clear any stale "encrypted" flag from a prior
            // target so boot-time restore doesn't wrap a plaintext adapter.
            let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
            Ok(adapter)
        }
    }

    /// Build the PLAIN (unwrapped) sync adapter described by `req` — the mobile
    /// twin of the desktop `build_adapter`. Validates the per-kind fields,
    /// resolves credentials (a non-empty inline value wins, else the stored
    /// keychain secret — the keychain-reuse contract), constructs the plugin
    /// init config, and opens the matching statically-embedded sync plugin.
    /// Does NOT probe, persist, or activate: the callers
    /// ([`Self::configure_sync_adapter_json`], [`Self::preview_sync_target_json`],
    /// [`Self::accept_remote_dataset_json`]) layer those on, so the one builder
    /// is shared across the configure + onboarding flows (no drift in the
    /// security-sensitive SFTP host-key gate).
    fn build_plain_sync_adapter(
        &self,
        req: &ConfigureSyncRequest,
    ) -> Result<Arc<dyn SyncAdapter>, StoreError> {
        match req.kind.as_str() {
            "local" => {
                let path = req.path.as_deref().unwrap_or_default().trim();
                if path.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "path".to_string(),
                        detail: "sync path must not be empty".to_string(),
                    });
                }
                let cfg = serde_json::json!({ "remote_root": path }).to_string();
                open_sync_plugin(&self.plugin_manager, PLUGIN_ID_SYNC_LOCAL, cfg)
            }
            "webdav" => {
                let url = req.url.as_deref().unwrap_or_default().trim();
                if url.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "url".to_string(),
                        detail: "WebDAV URL must not be empty".to_string(),
                    });
                }
                let user = req.user.as_deref().unwrap_or_default().trim();
                // Resolve the password: a non-empty request value wins (fresh
                // connect / re-typed in Settings); otherwise reuse the stored
                // keychain secret so URL-only edits don't require re-typing.
                // Empty == "no auth" (the desktop `build_adapter` contract).
                let resolved_password = match req.password.as_deref().map(str::trim) {
                    Some(p) if !p.is_empty() => Some(p.to_string()),
                    _ => self
                        .secret_store
                        .retrieve(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password)
                        .ok(),
                };
                let cfg = serde_json::json!({
                    "url": url,
                    "user": user,
                    "password": resolved_password.unwrap_or_default(),
                })
                .to_string();
                open_sync_plugin(&self.plugin_manager, PLUGIN_ID_WEBDAV, cfg)
            }
            "ftp" => {
                let host = req.host.as_deref().unwrap_or_default().trim();
                if host.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "host".to_string(),
                        detail: "FTP host must not be empty".to_string(),
                    });
                }
                let user = req.user.as_deref().unwrap_or_default().trim();
                if user.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "user".to_string(),
                        detail: "FTP user must not be empty".to_string(),
                    });
                }
                let port = req.port.unwrap_or(21);
                let path = req.path.as_deref().unwrap_or_default().trim();
                let mode = req.mode.as_deref().unwrap_or("explicit").trim();
                // The plugin re-validates + falls back to "explicit", but we
                // reject obviously-wrong modes here for a clear field error.
                if !matches!(mode, "implicit" | "explicit" | "plain") {
                    return Err(StoreError::InvalidField {
                        field: "mode".to_string(),
                        detail: format!("unknown FTPS mode: {mode}"),
                    });
                }
                // Same reuse contract as WebDAV, but FTP has no anonymous path
                // in our model: a missing keychain secret is an auth error.
                let resolved_password = match req.password.as_deref().map(str::trim) {
                    Some(p) if !p.is_empty() => p.to_string(),
                    _ => self
                        .secret_store
                        .retrieve(FTP_SECRET_ACCOUNT, SecretSlot::Password)
                        .map_err(|_| StoreError::Auth {
                            detail: "no FTP password configured".to_string(),
                        })?,
                };
                let cfg = serde_json::json!({
                    "host": host,
                    "port": port,
                    "user": user,
                    "password": resolved_password,
                    "path": path,
                    "mode": mode,
                })
                .to_string();
                open_sync_plugin(&self.plugin_manager, PLUGIN_ID_FTP, cfg)
            }
            "dropbox" => {
                let client_id = req.client_id.as_deref().unwrap_or_default().trim();
                if client_id.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "client_id".to_string(),
                        detail: "Dropbox client_id must not be empty".to_string(),
                    });
                }
                let client_secret = req.client_secret.as_deref().unwrap_or_default().trim();
                let path = req.path.as_deref().unwrap_or_default().trim();
                // The refresh token must already be in the keychain from a prior
                // `complete_sync_oauth_json` (the native auth session) — Dropbox
                // sync owns one managed slot, divorced from any account row.
                let refresh_token = self
                    .secret_store
                    .retrieve(DROPBOX_SECRET_ACCOUNT, SecretSlot::RefreshToken)
                    .map_err(|_| StoreError::Auth {
                        detail: "Dropbox sign-in required — no refresh token stored".to_string(),
                    })?;
                let cfg = serde_json::json!({
                    "client_id": client_id,
                    "client_secret": client_secret,
                    "base_path": path,
                    "refresh_token": refresh_token,
                })
                .to_string();
                open_sync_plugin(&self.plugin_manager, PLUGIN_ID_DROPBOX, cfg)
            }
            "googledrive" => {
                let client_id = req.client_id.as_deref().unwrap_or_default().trim();
                if client_id.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "client_id".to_string(),
                        detail: "Google Drive client_id must not be empty".to_string(),
                    });
                }
                let client_secret = req.client_secret.as_deref().unwrap_or_default().trim();
                // Google's token endpoint rejects exchanges without the secret,
                // so it's required here too (unlike Dropbox's optional secret).
                if client_secret.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "client_secret".to_string(),
                        detail: "Google Drive client_secret must not be empty".to_string(),
                    });
                }
                let folder_name = req.folder_name.as_deref().unwrap_or_default().trim();
                let refresh_token = self
                    .secret_store
                    .retrieve(GOOGLEDRIVE_SECRET_ACCOUNT, SecretSlot::RefreshToken)
                    .map_err(|_| StoreError::Auth {
                        detail: "Google Drive sign-in required — no refresh token stored"
                            .to_string(),
                    })?;
                let cfg = serde_json::json!({
                    "client_id": client_id,
                    "client_secret": client_secret,
                    "folder_name": folder_name,
                    "refresh_token": refresh_token,
                })
                .to_string();
                open_sync_plugin(&self.plugin_manager, PLUGIN_ID_GOOGLEDRIVE, cfg)
            }
            "sftp" => {
                let host = req.host.as_deref().unwrap_or_default().trim();
                if host.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "host".to_string(),
                        detail: "SFTP host must not be empty".to_string(),
                    });
                }
                let user = req.user.as_deref().unwrap_or_default().trim();
                if user.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "user".to_string(),
                        detail: "SFTP user must not be empty".to_string(),
                    });
                }
                let port = req.port.unwrap_or(22);
                let path = req.path.as_deref().unwrap_or_default().trim();
                if path.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "path".to_string(),
                        detail: "SFTP path must not be empty".to_string(),
                    });
                }
                let auth_method = req.auth_method.as_deref().unwrap_or("password").trim();
                // Resolve the auth credentials with the same keychain-reuse
                // contract as WebDAV/FTP. Password + key passphrase live in
                // separate slots so switching methods doesn't clobber either.
                let (resolved_password, resolved_key_path, resolved_key_passphrase) =
                    match auth_method {
                        "password" => {
                            let pw = match req.password.as_deref().map(str::trim) {
                                Some(p) if !p.is_empty() => p.to_string(),
                                _ => self
                                    .secret_store
                                    .retrieve(SFTP_SECRET_ACCOUNT, SecretSlot::Password)
                                    .map_err(|_| StoreError::Auth {
                                        detail: "no SFTP password configured".to_string(),
                                    })?,
                            };
                            (pw, String::new(), String::new())
                        }
                        "key" => {
                            let kp = req
                                .key_path
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .ok_or_else(|| StoreError::InvalidField {
                                    field: "key_path".to_string(),
                                    detail: "SSH key path must not be empty".to_string(),
                                })?
                                .to_string();
                            let pass = match req.key_passphrase.as_deref().map(str::trim) {
                                Some(p) if !p.is_empty() => p.to_string(),
                                _ => self
                                    .secret_store
                                    .retrieve(SFTP_KEY_SECRET_ACCOUNT, SecretSlot::Password)
                                    .ok()
                                    .unwrap_or_default(),
                            };
                            (String::new(), kp, pass)
                        }
                        other => {
                            return Err(StoreError::InvalidField {
                                field: "auth_method".to_string(),
                                detail: format!("unknown SFTP auth method: {other}"),
                            });
                        }
                    };
                // The user-pinned host fingerprint (§19.5 trust dialog) locks the
                // handshake to that exact key.
                let host_port = format!("{host}:{port}");
                let pinned_fp = UserPrefsHostKeyVerifier::new(self.db.shared())
                    .peek(&host_port)
                    .unwrap_or_default();
                // Enforce §19.5 in the BACKEND, not only the UI: refuse to
                // configure (and thus connect) an SFTP target whose host key isn't
                // pinned yet — an empty pin = silent TOFU (accept any key = MITM
                // exposure). The legitimate flow always trusts via
                // `trust_sftp_host_key` first, so this rejects only the unsafe
                // paths (a direct/refactored caller bypassing the trust dialog).
                if pinned_fp.trim().is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "pinned_fingerprint".to_string(),
                        detail: "SFTP host key not trusted yet — preview + trust \
                                 the host key first (§19.5)"
                            .to_string(),
                    });
                }
                let cfg = serde_json::json!({
                    "host": host,
                    "port": port,
                    "user": user,
                    "path": path,
                    "auth_method": auth_method,
                    "password": resolved_password,
                    "key_path": resolved_key_path,
                    "key_passphrase": resolved_key_passphrase,
                    "pinned_fingerprint": pinned_fp,
                })
                .to_string();
                open_sync_plugin(&self.plugin_manager, PLUGIN_ID_SFTP, cfg)
            }
            other => Err(StoreError::InvalidField {
                field: "kind".to_string(),
                detail: format!(
                    "sync adapter kind '{other}' is not supported \
                     (local, webdav, ftp, dropbox, googledrive, sftp)"
                ),
            }),
        }
    }

    /// Persist the device-local `sync.adapter.*` prefs + keychain secrets for a
    /// configured/joined target. The persist half of the old configure arms,
    /// split out so [`Self::configure_sync_adapter_json`] +
    /// [`Self::accept_remote_dataset_json`] persist identically. Re-derives the
    /// trimmed field values from `req` (cheap; the matching
    /// [`Self::build_plain_sync_adapter`] already validated them). Only a
    /// non-empty inline secret is written — an omitted/empty one keeps the
    /// stored keychain entry (the reuse contract). The `_` arm is unreachable
    /// (the builder rejects unknown kinds before this runs).
    fn persist_sync_config(&self, req: &ConfigureSyncRequest) -> Result<(), StoreError> {
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        match req.kind.as_str() {
            "local" => {
                let path = req.path.as_deref().unwrap_or_default().trim();
                prefs.set(PREF_ADAPTER_KIND, "local").map_err(storage_err)?;
                prefs.set(PREF_LOCAL_PATH, path).map_err(storage_err)?;
            }
            "webdav" => {
                let url = req.url.as_deref().unwrap_or_default().trim();
                let user = req.user.as_deref().unwrap_or_default().trim();
                prefs
                    .set(PREF_ADAPTER_KIND, "webdav")
                    .map_err(storage_err)?;
                prefs.set(PREF_WEBDAV_URL, url).map_err(storage_err)?;
                prefs.set(PREF_WEBDAV_USER, user).map_err(storage_err)?;
                if let Some(pw) = req.password.as_deref().map(str::trim) {
                    if !pw.is_empty() {
                        self.secret_store
                            .store(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password, pw)
                            .map_err(storage_err)?;
                    }
                }
            }
            "ftp" => {
                let host = req.host.as_deref().unwrap_or_default().trim();
                let user = req.user.as_deref().unwrap_or_default().trim();
                let port = req.port.unwrap_or(21);
                let path = req.path.as_deref().unwrap_or_default().trim();
                let mode = req.mode.as_deref().unwrap_or("explicit").trim();
                prefs.set(PREF_ADAPTER_KIND, "ftp").map_err(storage_err)?;
                prefs.set(PREF_FTP_HOST, host).map_err(storage_err)?;
                prefs
                    .set(PREF_FTP_PORT, &port.to_string())
                    .map_err(storage_err)?;
                prefs.set(PREF_FTP_USER, user).map_err(storage_err)?;
                prefs.set(PREF_FTP_PATH, path).map_err(storage_err)?;
                prefs.set(PREF_FTP_MODE, mode).map_err(storage_err)?;
                if let Some(pw) = req.password.as_deref().map(str::trim) {
                    if !pw.is_empty() {
                        self.secret_store
                            .store(FTP_SECRET_ACCOUNT, SecretSlot::Password, pw)
                            .map_err(storage_err)?;
                    }
                }
            }
            "dropbox" => {
                let client_id = req.client_id.as_deref().unwrap_or_default().trim();
                let client_secret = req.client_secret.as_deref().unwrap_or_default().trim();
                let path = req.path.as_deref().unwrap_or_default().trim();
                prefs
                    .set(PREF_ADAPTER_KIND, "dropbox")
                    .map_err(storage_err)?;
                prefs
                    .set(PREF_DROPBOX_CLIENT_ID, client_id)
                    .map_err(storage_err)?;
                prefs
                    .set(PREF_DROPBOX_CLIENT_SECRET, client_secret)
                    .map_err(storage_err)?;
                prefs.set(PREF_DROPBOX_PATH, path).map_err(storage_err)?;
            }
            "googledrive" => {
                let client_id = req.client_id.as_deref().unwrap_or_default().trim();
                let client_secret = req.client_secret.as_deref().unwrap_or_default().trim();
                let folder_name = req.folder_name.as_deref().unwrap_or_default().trim();
                prefs
                    .set(PREF_ADAPTER_KIND, "googledrive")
                    .map_err(storage_err)?;
                prefs
                    .set(PREF_GOOGLEDRIVE_CLIENT_ID, client_id)
                    .map_err(storage_err)?;
                prefs
                    .set(PREF_GOOGLEDRIVE_CLIENT_SECRET, client_secret)
                    .map_err(storage_err)?;
                prefs
                    .set(PREF_GOOGLEDRIVE_FOLDER_NAME, folder_name)
                    .map_err(storage_err)?;
            }
            "sftp" => {
                let host = req.host.as_deref().unwrap_or_default().trim();
                let user = req.user.as_deref().unwrap_or_default().trim();
                let port = req.port.unwrap_or(22);
                let path = req.path.as_deref().unwrap_or_default().trim();
                let auth_method = req.auth_method.as_deref().unwrap_or("password").trim();
                prefs.set(PREF_ADAPTER_KIND, "sftp").map_err(storage_err)?;
                prefs.set(PREF_SFTP_HOST, host).map_err(storage_err)?;
                prefs
                    .set(PREF_SFTP_PORT, &port.to_string())
                    .map_err(storage_err)?;
                prefs.set(PREF_SFTP_USER, user).map_err(storage_err)?;
                prefs.set(PREF_SFTP_PATH, path).map_err(storage_err)?;
                prefs
                    .set(PREF_SFTP_AUTH_METHOD, auth_method)
                    .map_err(storage_err)?;
                if auth_method == "key" {
                    let key_path = req.key_path.as_deref().unwrap_or_default().trim();
                    prefs
                        .set(PREF_SFTP_KEY_PATH, key_path)
                        .map_err(storage_err)?;
                    if let Some(pp) = req.key_passphrase.as_deref().map(str::trim) {
                        if !pp.is_empty() {
                            self.secret_store
                                .store(SFTP_KEY_SECRET_ACCOUNT, SecretSlot::Password, pp)
                                .map_err(storage_err)?;
                        }
                    }
                } else if let Some(pw) = req.password.as_deref().map(str::trim) {
                    if !pw.is_empty() {
                        self.secret_store
                            .store(SFTP_SECRET_ACCOUNT, SecretSlot::Password, pw)
                            .map_err(storage_err)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Task-list twin of [`Host::route`]: `None` is the local branch (the
    /// `LocalAdapter`), `Some(ext)` an external task provider. An unknown id is
    /// treated as local (the desktop `account_for_task_list().unwrap_or(LOCAL)`
    /// fallback); a non-local id whose adapter isn't live is `NotFound`.
    ///
    /// Routing relies on the list→account map, which [`Host::task_lists_json`]
    /// primes — callers list task lists before task/section ops (the desktop
    /// invariant).
    fn route_task_list(&self, list_id: &str) -> Result<Option<Arc<dyn TasksFeature>>, StoreError> {
        let account = self
            .registry
            .account_for_task_list(list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        if account == LOCAL_ID {
            Ok(None)
        } else {
            self.registry
                .task_adapter(&account)
                .map(Some)
                .ok_or(StoreError::NotFound)
        }
    }

    /// Whether `list_id` belongs to the local account (unknown → local, matching
    /// `route_task_list`). Gates the append-to-event-log decision for task/list/
    /// section mutations: only LOCAL ones are logged for sync; external
    /// providers self-sync.
    fn is_local_task_list(&self, list_id: &str) -> bool {
        self.registry
            .account_for_task_list(list_id)
            .is_none_or(|a| a == LOCAL_ID)
    }

    /// Contacts twin of [`Host::route_task_list`]: `None` is the local address
    /// book, `Some(ext)` an external contacts provider. Unknown id → local;
    /// a non-local id whose adapter isn't live is `NotFound`. Contacts are NOT
    /// event-logged (no `Contact*` SyncEvent), so there's no is_local gate —
    /// local contacts are device-local, external ones self-sync via the provider.
    /// [`Host::contact_lists_json`] primes the list→account route map.
    fn route_contact_list(
        &self,
        list_id: &str,
    ) -> Result<Option<Arc<dyn ContactsFeature>>, StoreError> {
        let account = self
            .registry
            .account_for_contact_list(list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        if account == LOCAL_ID {
            Ok(None)
        } else {
            self.registry
                .contact_adapter(&account)
                .map(Some)
                .ok_or(StoreError::NotFound)
        }
    }

    /// Resolve an account's task capabilities from its plugin manifest (mirrors
    /// the desktop `task_caps_for_account`): the local store reports its own
    /// caps; an account whose plugin we can't resolve falls back to the
    /// permissive cal-core-native default.
    fn task_caps_for_account(
        &self,
        account_id: &str,
        account_kinds: &std::collections::HashMap<String, AdapterKind>,
    ) -> TaskCapabilities {
        if account_id == LOCAL_ID {
            return local_task_capabilities();
        }
        let Some(plugin_id) = account_kinds.get(account_id).and_then(|k| k.plugin_id()) else {
            return TaskCapabilities::default();
        };
        self.plugin_manager
            .get_including_disabled(plugin_id)
            .map(|p| p.manifest.tasks.clone())
            .unwrap_or_default()
    }
}

/// A contact list enriched with its owning `account_id` — mirrors the desktop
/// wire shape (and `TaskListRow`). Lets the UI tell local (deletable) from
/// external (provider-managed) address books.
#[derive(serde::Serialize)]
struct ContactListRow {
    #[serde(flatten)]
    inner: ContactList,
    account_id: String,
}

/// A task list enriched with its owning `account_id` + the adapter's
/// `task_capabilities` — the desktop `TaskListRow` wire shape. The mobile UI
/// gates affordances (recurrence, sections, …) on the capabilities, so an
/// external list that can't store a recurrence rule no longer offers it then
/// silently drops it on save.
#[derive(serde::Serialize)]
struct TaskListRow {
    #[serde(flatten)]
    inner: TaskList,
    account_id: String,
    task_capabilities: TaskCapabilities,
}

/// The local SQLite store's task capabilities — it has no manifest, so hard-code
/// what it actually supports (nested lists + sections, on top of the cal-core
/// default's subtasks/recurrence/cross-list-move). Mirrors the desktop
/// `local_task_capabilities`.
fn local_task_capabilities() -> TaskCapabilities {
    TaskCapabilities {
        nested_projects: true,
        sections: true,
        manageable_sections: true,
        create_lists: true,
        delete_lists: true,
        ..TaskCapabilities::default()
    }
}

/// Open a statically-embedded sync-adapter plugin instance + wrap it as a
/// `SyncAdapter` (the desktop `open_sync_plugin` pattern). Free fn so both
/// `configure_sync_adapter_json` and the restore-on-open path (which runs
/// before `Host` exists) can call it.
fn open_sync_plugin(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    config_json: String,
) -> Result<Arc<dyn SyncAdapter>, StoreError> {
    let plugin = plugin_manager
        .get(plugin_id)
        .ok_or_else(|| StoreError::Storage {
            detail: format!("sync plugin {plugin_id} is not loaded"),
        })?;
    let instance = plugin_manager
        .open_instance(plugin, &config_json)
        .map_err(|e| StoreError::Storage {
            detail: format!("open sync plugin {plugin_id}: {e}"),
        })?;
    let adapter = FfiSyncAdapter::new(instance).ok_or_else(|| StoreError::Storage {
        detail: format!("plugin {plugin_id} has no SyncAdapter surface"),
    })?;
    Ok(Arc::new(adapter))
}

/// Reconstruct the configured sync adapter from `user_prefs` — the mobile twin
/// of the desktop `build_adapter_from_prefs`. Runs in [`Host::open`] before
/// `Self` exists, so it takes the plugin manager + secret store by reference.
/// Best-effort: a missing/blank field or an open failure yields `None`, leaving
/// sync unconfigured until the user re-configures from the Sync screen. Restores
/// every kind this host can configure (`local`/`webdav`/`ftp`/`dropbox`/
/// `googledrive`/`sftp`). `shared` is taken alongside `prefs` so the `sftp` arm
/// can read the pinned host-key fingerprint via the shared verifier.
fn restore_adapter_from_prefs(
    shared: &SharedConn,
    prefs: &UserPrefsRepo,
    plugin_manager: &PluginManager,
    secret_store: &dyn SecretStore,
) -> Option<Arc<dyn SyncAdapter>> {
    let kind = prefs.get(PREF_ADAPTER_KIND).ok().flatten()?;
    match kind.as_str() {
        "local" => {
            let path = prefs.get(PREF_LOCAL_PATH).ok().flatten()?;
            if path.trim().is_empty() {
                return None;
            }
            let cfg = serde_json::json!({ "remote_root": path.trim() }).to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_SYNC_LOCAL, cfg).ok()
        }
        "webdav" => {
            let url = prefs.get(PREF_WEBDAV_URL).ok().flatten()?;
            if url.trim().is_empty() {
                return None;
            }
            let user = prefs
                .get(PREF_WEBDAV_USER)
                .ok()
                .flatten()
                .unwrap_or_default();
            // Missing keychain entry → empty password (the WebDAV adapter
            // treats that as "no auth", matching the desktop restore path).
            let password = secret_store
                .retrieve(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password)
                .ok()
                .unwrap_or_default();
            let cfg = serde_json::json!({
                "url": url.trim(),
                "user": user.trim(),
                "password": password,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_WEBDAV, cfg).ok()
        }
        "ftp" => {
            let host = prefs.get(PREF_FTP_HOST).ok().flatten()?;
            if host.trim().is_empty() {
                return None;
            }
            let port = prefs
                .get(PREF_FTP_PORT)
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(21);
            let user = prefs.get(PREF_FTP_USER).ok().flatten()?;
            if user.trim().is_empty() {
                return None;
            }
            let path = prefs.get(PREF_FTP_PATH).ok().flatten().unwrap_or_default();
            let mode = prefs
                .get(PREF_FTP_MODE)
                .ok()
                .flatten()
                .unwrap_or_else(|| "explicit".to_string());
            // FTP has no anonymous path in our model — a missing password
            // means the config is incomplete, so don't restore (the desktop
            // `?`-shortcuts here too).
            let password = secret_store
                .retrieve(FTP_SECRET_ACCOUNT, SecretSlot::Password)
                .ok()?;
            let cfg = serde_json::json!({
                "host": host.trim(),
                "port": port,
                "user": user.trim(),
                "password": password,
                "path": path.trim(),
                "mode": mode,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_FTP, cfg).ok()
        }
        "dropbox" => {
            let client_id = prefs.get(PREF_DROPBOX_CLIENT_ID).ok().flatten()?;
            if client_id.trim().is_empty() {
                return None;
            }
            let client_secret = prefs
                .get(PREF_DROPBOX_CLIENT_SECRET)
                .ok()
                .flatten()
                .unwrap_or_default();
            let base_path = prefs
                .get(PREF_DROPBOX_PATH)
                .ok()
                .flatten()
                .unwrap_or_default();
            // No refresh token → the connection is incomplete (sign-in needed);
            // don't restore, leaving sync unconfigured until the user re-connects.
            let refresh_token = secret_store
                .retrieve(DROPBOX_SECRET_ACCOUNT, SecretSlot::RefreshToken)
                .ok()?;
            let cfg = serde_json::json!({
                "client_id": client_id.trim(),
                "client_secret": client_secret.trim(),
                "base_path": base_path.trim(),
                "refresh_token": refresh_token,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_DROPBOX, cfg).ok()
        }
        "googledrive" => {
            let client_id = prefs.get(PREF_GOOGLEDRIVE_CLIENT_ID).ok().flatten()?;
            if client_id.trim().is_empty() {
                return None;
            }
            let client_secret = prefs
                .get(PREF_GOOGLEDRIVE_CLIENT_SECRET)
                .ok()
                .flatten()
                .unwrap_or_default();
            // Google requires the secret at every token call — incomplete without it.
            if client_secret.trim().is_empty() {
                return None;
            }
            let folder_name = prefs
                .get(PREF_GOOGLEDRIVE_FOLDER_NAME)
                .ok()
                .flatten()
                .unwrap_or_default();
            let refresh_token = secret_store
                .retrieve(GOOGLEDRIVE_SECRET_ACCOUNT, SecretSlot::RefreshToken)
                .ok()?;
            let cfg = serde_json::json!({
                "client_id": client_id.trim(),
                "client_secret": client_secret.trim(),
                "folder_name": folder_name.trim(),
                "refresh_token": refresh_token,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_GOOGLEDRIVE, cfg).ok()
        }
        "sftp" => {
            let host = prefs.get(PREF_SFTP_HOST).ok().flatten()?;
            if host.trim().is_empty() {
                return None;
            }
            let port = prefs
                .get(PREF_SFTP_PORT)
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(22);
            let user = prefs.get(PREF_SFTP_USER).ok().flatten()?;
            if user.trim().is_empty() {
                return None;
            }
            let path = prefs.get(PREF_SFTP_PATH).ok().flatten()?;
            if path.trim().is_empty() {
                return None;
            }
            let auth_method = prefs
                .get(PREF_SFTP_AUTH_METHOD)
                .ok()
                .flatten()
                .unwrap_or_else(|| "password".to_string());
            let (password, key_path, key_passphrase) = match auth_method.as_str() {
                "key" => {
                    let kp = prefs
                        .get(PREF_SFTP_KEY_PATH)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    if kp.trim().is_empty() {
                        return None;
                    }
                    // A missing passphrase is fine (unencrypted key) → empty.
                    let pass = secret_store
                        .retrieve(SFTP_KEY_SECRET_ACCOUNT, SecretSlot::Password)
                        .ok()
                        .unwrap_or_default();
                    (String::new(), kp.trim().to_string(), pass)
                }
                // "password" + any unknown value both resolve as password auth —
                // forward-compat parity with the desktop build_adapter_from_prefs
                // restore arm (a future auth method an older Aperio doesn't know
                // still restores as password rather than silently dropping sync).
                // No stored password → incomplete; don't restore.
                _ => {
                    let pw = secret_store
                        .retrieve(SFTP_SECRET_ACCOUNT, SecretSlot::Password)
                        .ok()?;
                    (pw, String::new(), String::new())
                }
            };
            let host_port = format!("{}:{port}", host.trim());
            let pinned_fp = UserPrefsHostKeyVerifier::new(shared.clone())
                .peek(&host_port)
                .unwrap_or_default();
            // §19.5: never restore an SFTP target with no pinned host key — that
            // would silently TOFU (accept whatever key the network presents) on
            // the next sync. Leave sync unconfigured until the user re-trusts via
            // the Sync screen.
            if pinned_fp.trim().is_empty() {
                return None;
            }
            let cfg = serde_json::json!({
                "host": host.trim(),
                "port": port,
                "user": user.trim(),
                "path": path.trim(),
                "auth_method": auth_method,
                "password": password,
                "key_path": key_path,
                "key_passphrase": key_passphrase,
                "pinned_fingerprint": pinned_fp,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_SFTP, cfg).ok()
        }
        _ => None,
    }
}

#[uniffi::export]
impl Host {
    /// Open the on-device database at `db_path`, register every bundled
    /// adapter plugin statically, build the adapter registry over the
    /// supplied keychain bridge, and materialise adapters for the
    /// persisted accounts.
    #[uniffi::constructor]
    pub fn open(
        db_path: String,
        keychain: Arc<dyn KeychainBridge>,
    ) -> Result<Arc<Self>, StoreError> {
        let db = DbHandle::open(&db_path).map_err(|e| StoreError::Open {
            detail: e.to_string(),
        })?;

        // The local adapter shares the single writer connection with the
        // registry's external adapters — the same topology as the desktop
        // backend (one DbHandle, many Arc clones of its mutex).
        let adapter = LocalAdapter::new(db.shared());

        // One-worker multi-thread runtime: `block_on` drives the
        // CalendarFeature methods + sync rounds, while the event-log writer's
        // drain task lives on the worker thread and keeps flushing appends
        // between our calls. `enable_all` gives the time + I/O drivers the
        // external-adapter shim's HTTP + the writer need.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| StoreError::Open {
                detail: format!("tokio runtime: {e}"),
            })?;

        // Static plugin embedding: no dlopen (iOS forbids it) — the 17
        // `-plugin` rlibs are linked into this library and registered by id.
        let plugin_manager = Arc::new(PluginManager::new(env!("CARGO_PKG_VERSION")));
        host_plugins::register_all_static(&plugin_manager).map_err(|e| StoreError::Open {
            detail: format!("plugin registration failed: {e}"),
        })?;

        let secret_store: Arc<dyn SecretStore> = Arc::new(BridgeSecretStore { bridge: keychain });

        // The app-sandbox data dir (where aperio.sqlite lives): per-account
        // plugin state (EWS sync cookies) + the sync pending-log tree both hang
        // off it. Always Some on a real device path; `.` only on a bare file
        // name (tests pass an absolute temp path).
        let data_dir = std::path::Path::new(&db_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let registry = Arc::new(AdapterRegistry::with_data_dir(
            Arc::clone(&plugin_manager),
            Arc::clone(&secret_store),
            Some(data_dir.clone()),
        ));
        {
            let shared = db.shared();
            let repo = AccountsRepo::new(&shared);
            registry.bootstrap(&repo);
        }

        // The sync graph — the SAME assembly the desktop builds
        // (host_core::sync::build_orchestrator), over our keychain-bridged
        // SecretStore. Built inside the runtime so the writer's drain task has
        // a context. ONE boot_at, shared by writer + orchestrator.
        let boot_at = chrono::Utc::now();
        let graph = runtime.block_on(async {
            build_orchestrator(
                db.shared(),
                data_dir,
                Arc::clone(&secret_store),
                env!("CARGO_PKG_VERSION"),
                boot_at,
            )
        });

        // Restore a previously-configured sync adapter so `sync_now` works
        // without a re-configure step (the desktop's build_adapter_from_prefs).
        // Best-effort: a missing/unbuildable adapter just leaves sync
        // unconfigured (the user re-configures from the Sync screen).
        {
            let shared = db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            if let Some(plain) =
                restore_adapter_from_prefs(&shared, &prefs, &plugin_manager, secret_store.as_ref())
            {
                // §19.7: if the dataset is E2E, wrap with the device-local key.
                // When the flag is set but the key is missing (keychain wiped /
                // fresh OS install with the same data dir), DON'T configure — we
                // must never push plaintext to an encrypted target; the user
                // re-enters the passphrase to re-establish the key.
                let configured: Option<Arc<dyn SyncAdapter>> =
                    if host_core::credential_sync::e2e_enabled(&shared) {
                        load_e2e_key(secret_store.as_ref())
                            .map(|key| wrap_if_encrypted(plain, Some(key)))
                    } else {
                        Some(plain)
                    };
                if let Some(adapter) = configured {
                    graph.orchestrator.configure(adapter);
                }
            }
        }

        Ok(Arc::new(Self {
            db,
            adapter,
            registry,
            secret_store,
            runtime,
            writer: graph.writer,
            orchestrator: graph.orchestrator,
            onboarding: graph.onboarding,
            plugin_manager,
            progress: SyncProgressDriver::default(),
        }))
    }

    /// All persisted accounts as JSON (the `cal_core`/desktop wire shape).
    pub fn accounts_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let accounts = repo.list().map_err(acc_err)?;
        to_json(&accounts)
    }

    /// Create an external (or local) account: persist the row, store the
    /// secret via the keychain bridge, and register the adapter so
    /// subsequent reads/writes route through it. Returns the created
    /// account as JSON.
    ///
    /// Mirrors the desktop `create_account` minus the pre-persist
    /// credential smoke-test + the cross-device credential/account
    /// sync-log events (those need the event log + a tokio runtime and
    /// arrive with the read/write/sync phases). Like the desktop, a
    /// secret-store or registration failure tears the row back down so
    /// the DB, keychain, and registry never drift.
    pub fn create_account_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: NewAccountRequest = from_json("account", &request_json)?;

        // Only the kinds with a non-OAuth construction path. Google /
        // Microsoft Graph / the VC kinds need the interactive OAuth flow
        // (a later phase); reject them here rather than persisting a row
        // that can never authenticate.
        if !matches!(
            req.adapter_kind,
            AdapterKind::Local
                | AdapterKind::Caldav
                | AdapterKind::Ical
                | AdapterKind::Ews
                | AdapterKind::Vikunja
                | AdapterKind::Todoist
        ) {
            return Err(StoreError::InvalidField {
                field: "adapter_kind".to_string(),
                detail: format!(
                    "adapter '{}' needs the interactive OAuth flow (a later phase)",
                    req.adapter_kind.as_str()
                ),
            });
        }

        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let created = repo
            .create(req.adapter_kind, req.display_name.trim(), &req.config_json)
            .map_err(acc_err)?;

        // Persist the secret right after the row so the keychain and DB
        // stay aligned. The slot mirrors what the registry's register_*
        // path reads back: API token for Vikunja/Todoist, password
        // otherwise. A write failure is fatal — tear the row down.
        if let Some(secret) = req.secret {
            let slot = match req.adapter_kind {
                AdapterKind::Vikunja | AdapterKind::Todoist => SecretSlot::ApiToken,
                _ => SecretSlot::Password,
            };
            if let Err(err) = self.secret_store.store(&created.id, slot, &secret) {
                let _ = repo.delete(&created.id);
                return Err(StoreError::Storage {
                    detail: format!("failed to store credential: {err}"),
                });
            }
            // E2E only: also push the secret to the user's other devices via the
            // encrypted log so the account works there without re-entry. A no-op
            // (gated) when E2E is off — credentials then stay device-local.
            host_core::credential_sync::emit_credential_set(
                &self.writer,
                &shared,
                &created.id,
                slot,
                &secret,
            );
        }

        // Register the freshly created external adapter. A failure is
        // fatal: drop the secrets + row so keychain/DB/registry stay in step.
        if req.adapter_kind != AdapterKind::Local {
            if let Err(err) = self.registry.register(&created) {
                let _ = self.secret_store.delete_all(&created.id);
                let _ = repo.delete(&created.id);
                return Err(StoreError::Storage {
                    detail: format!("adapter registration failed: {err}"),
                });
            }
        }

        // Sync the new account row to other devices (non-secret metadata only;
        // the receiver surfaces the "reconnect" wizard for the device-local
        // secret). Mirrors the desktop create_account.
        self.writer
            .append(SyncEvent::AccountCreated(account_payload(&created)));

        to_json(&created)
    }

    /// Delete an account: unregister its adapter, clear its secrets, and
    /// remove the row. The local account cannot be deleted
    /// ([`StoreError::InvalidField`]).
    pub fn delete_account(&self, account_id: String) -> Result<(), StoreError> {
        self.registry.unregister(&account_id);
        let _ = self.secret_store.delete_all(&account_id);
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        repo.delete(&account_id).map_err(acc_err)?;
        // Propagate the deletion to other devices (cascades secrets there too).
        self.writer
            .append(SyncEvent::AccountDeleted(IdPayload { id: account_id }));
        Ok(())
    }

    // ─── Calendars ───────────────────────────────────────────────────────────

    /// All calendars (local + external) as a JSON `CalendarRow[]`, and — as a
    /// side effect — primes the registry's calendar→account route map so the
    /// event methods can route. Mirrors the desktop `list_calendars` minus the
    /// SWR cache, overrides, birthday calendars, and recurrence-capability
    /// resolution (all deferred). Callers should list calendars before event
    /// operations — the same ordering the desktop frontend honours.
    pub fn list_calendars_json(&self) -> Result<String, StoreError> {
        // Fetch real calendars (+ their accounts) and the synthetic birthday rows.
        let (mut calendars, accounts, birthday_rows) = self.runtime.block_on(async {
            let local = self.adapter.list_calendars().await.map_err(map_store_err)?;
            for c in &local {
                self.registry.note_calendar_route(&c.id, LOCAL_ID);
            }
            // `list_external_calendars` stamps external routes internally and
            // swallows per-adapter errors so one dead account can't blank the
            // whole list.
            let external = self.registry.list_external_calendars().await;

            let mut calendars: Vec<Calendar> = Vec::with_capacity(local.len() + external.len());
            let mut accounts: Vec<String> = Vec::with_capacity(local.len() + external.len());
            for c in local {
                accounts.push(LOCAL_ID.to_string());
                calendars.push(c);
            }
            for c in external {
                accounts.push(
                    self.registry
                        .account_for_calendar(&c.id)
                        .unwrap_or_else(|| LOCAL_ID.to_string()),
                );
                calendars.push(c);
            }

            // Synthetic, read-only birthday calendars (§10.3) from the LOCAL
            // address book — one per contact list that has ≥1 birthday. External
            // birthday layers need the snapshot contact cache the mobile host
            // doesn't have yet, so the local books are the ported subset; the
            // events are synthesised on demand in get_events_json. Stamp each
            // id's route as local so a stray lookup resolves locally. (No
            // overrides apply to synthetic calendars.)
            let mut birthday_rows: Vec<CalendarRow> = Vec::new();
            if let Ok(contact_lists) = self.adapter.list_contact_lists().await {
                for list in contact_lists {
                    let has_birthday = self
                        .adapter
                        .get_contacts(&list.id)
                        .await
                        .map(|cs| cs.iter().any(|c| c.birthday.is_some()))
                        .unwrap_or(false);
                    if has_birthday {
                        let cal = host_core::birthdays::synthesise_calendar(&list.id, &list.name);
                        self.registry.note_calendar_route(&cal.id, LOCAL_ID);
                        birthday_rows.push(CalendarRow::new(cal, LOCAL_ID.to_string()));
                    }
                }
            }
            Ok::<_, StoreError>((calendars, accounts, birthday_rows))
        })?;

        // Stamp host-local name + colour overrides (external containers + local
        // contact lists; a local calendar carries its own binding + has no
        // override row, so this no-ops for it).
        {
            let shared = self.db.shared();
            let repo = OverridesRepo::new(&shared);
            apply_to_calendars(&repo, &mut calendars);
            apply_color_to_calendars(&repo, &mut calendars);
        }

        let mut rows: Vec<CalendarRow> = calendars
            .into_iter()
            .zip(accounts)
            .map(|(c, account)| CalendarRow::new(c, account))
            .collect();
        rows.extend(birthday_rows);
        to_json(&rows)
    }

    /// Create a local calendar; returns it as a `CalendarRow`. Stamps the new
    /// id's route immediately so a following event op routes without a
    /// re-list. Local-only (the adapter surface has no external-calendar
    /// creation); colour/sound are deferred (always `None` here).
    pub fn create_calendar_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: CreateCalendarRequest = from_json("calendar", &request_json)?;
        // Pass the name verbatim — the desktop `create_calendar` does not trim,
        // and neither does LocalAdapter; trimming here would diverge the stored
        // value from the desktop for the same input.
        let created = self
            .adapter
            .create_calendar(&req.name, None, req.color_label.map(ColorLabelId), None)
            .map_err(map_store_err)?;
        self.registry.note_calendar_route(&created.id, LOCAL_ID);
        // Local-only → always log for sync.
        if let Ok(fields) = serde_json::to_value(&created) {
            self.writer.append(SyncEvent::CalendarCreated(EventPayload {
                id: created.id.clone(),
                fields,
            }));
        }
        to_json(&CalendarRow::new(created, LOCAL_ID.to_string()))
    }

    /// Delete a local calendar (its events cascade away). Mirrors the desktop
    /// local-only `delete_calendar`.
    pub fn delete_calendar(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_calendar(&id).map_err(map_store_err)?;
        self.writer
            .append(SyncEvent::CalendarDeleted(IdPayload { id }));
        Ok(())
    }

    // ─── Events ──────────────────────────────────────────────────────────────

    /// Events in `calendar_id` overlapping `[start, end]`, as a JSON `Event[]`.
    /// Routes local → LocalAdapter, external → the registry adapter. Mirrors
    /// the desktop `get_events` minus the SWR read-cache + staleness-gated
    /// background refresh (deferred): the external branch hits the provider
    /// live, exactly as a cache-cold desktop first read. A synthetic birthday
    /// calendar id is intercepted first and its all-day events are synthesised
    /// on the fly from the LOCAL contacts' birthdays (§10.3).
    ///
    /// The local adapter currently returns rows whose stored start/end
    /// intersect the range (RRULE occurrence expansion is its own later phase),
    /// so a recurring master is returned only when its stored span overlaps.
    pub fn get_events_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: EventRangeRequest = from_json("request", &request_json)?;
        let range = DateRange::new(req.start, req.end);
        // Only EXTERNAL events get the host-local colour stamp (local events carry
        // their own binding; synthetic birthday events need none).
        let is_external = !host_core::birthdays::is_birthday_calendar_id(&req.calendar_id)
            && self.route(&req.calendar_id)?.is_some();
        let mut events = self.runtime.block_on(async {
            // Birthday calendars are synthesised, not stored: derive their
            // events from the underlying LOCAL contact list's birthdays. (The
            // mobile host has no external contact cache, so only local birthday
            // layers exist — see list_calendars_json.)
            if host_core::birthdays::is_birthday_calendar_id(&req.calendar_id) {
                let list_id = host_core::birthdays::underlying_contact_list_id(&req.calendar_id)
                    .unwrap_or_default();
                let contacts = self.adapter.get_contacts(list_id).await.unwrap_or_default();
                return Ok::<_, StoreError>(host_core::birthdays::events_for_contacts(
                    contacts,
                    &req.calendar_id,
                    range,
                ));
            }
            match self.route(&req.calendar_id)? {
                None => self
                    .adapter
                    .get_events(&req.calendar_id, range)
                    .await
                    .map_err(map_store_err),
                Some(ext) => ext
                    .get_events(&req.calendar_id, range)
                    .await
                    .map_err(map_store_err),
            }
        })?;
        if is_external {
            // Map a colour-capable provider's native color_hex back to a label
            // FIRST, then stamp host-local overrides (which skip native-coloured
            // events) — the desktop ordering so a provider colour always wins.
            for ev in events.iter_mut() {
                if let Some(hex) = ev.color_hex.clone() {
                    if let Ok(Some(label)) = self.adapter.match_hex_to_label(&hex) {
                        ev.color_label = Some(ColorLabelId(label));
                    }
                }
            }
            let shared = self.db.shared();
            let repo = OverridesRepo::new(&shared);
            apply_color_to_events(&repo, &mut events);
        }
        to_json(&events)
    }

    /// One local event by id as JSON (`Event` or `null`). Local-only by design
    /// — the desktop `get_event_by_id` is the reminders-overview lookup against
    /// the local store; external events aren't addressable by a bare id without
    /// their calendar.
    pub fn get_event_by_id_json(&self, id: String) -> Result<String, StoreError> {
        let event = self.adapter.get_event_by_id(&id).map_err(map_store_err)?;
        to_json(&event)
    }

    /// Create an event in `calendar_id` from a flattened `NewEvent`; returns
    /// the created `Event` as JSON. Routes local/external. Mirrors the desktop
    /// `create_event` minus colour resolution + reminder reschedule (deferred).
    /// A LOCAL create is logged to the event log so the next sync round carries
    /// it; an external create self-syncs via the provider.
    pub fn create_event_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: CreateEventRequest = from_json("request", &request_json)?;
        let created = self.runtime.block_on(async {
            match self.route(&req.calendar_id)? {
                None => self
                    .adapter
                    .create_event(&req.calendar_id, req.event)
                    .await
                    .map_err(map_store_err),
                Some(ext) => ext
                    .create_event(&req.calendar_id, req.event)
                    .await
                    .map_err(map_store_err),
            }
        })?;
        if self.is_local_calendar(&created.calendar_id) {
            if let Ok(fields) = serde_json::to_value(&created) {
                self.writer.append(SyncEvent::EventCreated(EventPayload {
                    id: created.id.clone(),
                    fields,
                }));
            }
        }
        to_json(&created)
    }

    /// Update an event in place (its `calendar_id` field selects the route);
    /// returns the updated `Event` as JSON. Mirrors the in-place branch of the
    /// desktop `update_event`. Cross-calendar moves (the create-on-target +
    /// best-effort-delete dance with `previous_calendar_id`) are deferred.
    pub fn update_event_json(&self, event_json: String) -> Result<String, StoreError> {
        let event: Event = from_json("event", &event_json)?;
        let updated = self.runtime.block_on(async {
            match self.route(&event.calendar_id)? {
                None => self
                    .adapter
                    .update_event(event)
                    .await
                    .map_err(map_store_err),
                Some(ext) => ext.update_event(event).await.map_err(map_store_err),
            }
        })?;
        if self.is_local_calendar(&updated.calendar_id) {
            if let Ok(fields) = serde_json::to_value(&updated) {
                self.writer.append(SyncEvent::EventUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
        }
        to_json(&updated)
    }

    /// Delete an event. `calendar_id` is routing-only (dropped before the
    /// adapter call); omitted → assume local (desktop back-compat).
    /// `send_cancellations` (external only) defaults to false; local has no
    /// attendees so it never sends. Mirrors the desktop `delete_event`.
    pub fn delete_event(
        &self,
        id: String,
        calendar_id: Option<String>,
        send_cancellations: Option<bool>,
    ) -> Result<(), StoreError> {
        let send = send_cancellations.unwrap_or(false);
        // Omitted calendar_id → assume local (desktop back-compat).
        let is_local = calendar_id
            .as_deref()
            .is_none_or(|cid| self.is_local_calendar(cid));
        self.runtime.block_on(async {
            let route = match calendar_id.as_deref() {
                Some(cid) => self.route(cid)?,
                None => None,
            };
            match route {
                None => self
                    .adapter
                    .delete_event(&id, false)
                    .await
                    .map_err(map_store_err),
                Some(ext) => ext.delete_event(&id, send).await.map_err(map_store_err),
            }
        })?;
        if is_local {
            self.writer
                .append(SyncEvent::EventDeleted(IdPayload { id }));
        }
        Ok(())
    }

    // ── Tasks / lists / sections (JSON bridge, sync-logged) ───────────────────
    //
    // The faithful tasks port lives on the Host (folded in from the original
    // `LocalStore`). READS route local + external (the desktop
    // `account_for_task_list` split, like the event `route()`): `task_lists_json`
    // merges local lists with every external task account's, `tasks_json` /
    // `sections_json` route by the list's owning account. So a Vikunja/Todoist/
    // CalDAV-tasks account's lists + tasks + sections are now VISIBLE on mobile.
    //
    // WRITES route too (mirroring `commands::tasks`): a LOCAL mutation hits the
    // store and appends the matching `SyncEvent` (`EventPayload { id,
    // to_value(&entity) }` for Created/Updated, `IdPayload { id }` for Deleted —
    // which is what syncs it cross-device); an EXTERNAL mutation is routed to the
    // provider, which self-syncs, so NO event is logged. `delete_task` /
    // `delete_section` carry an optional `list_id` for routing (the desktop
    // `delete_task` shape). The JSON wire is the `cal_core` serde shape, so
    // `@aperio/shared` parses both sides.
    //
    // Mobile-deferred (documented per method): `create_task_list` stays LOCAL
    // (external list creation needs account + parent params this signature lacks)
    // and `reparent_task_list` is a local-store concept (external lists →
    // `Unsupported`); external recurring tasks get no host-side series_id +
    // on-demand next-instance spawn (cache-dependent); cross-list MOVES aren't
    // detected (no previous_list_id); cache-invalidation + the reminder scheduler
    // kick are desktop-only and have no mobile analogue.

    /// All task lists (local + external) as a JSON `TaskListRow[]` (the desktop
    /// wire shape: each `TaskList` flattened + its `account_id`). Primes the
    /// list→account route map for the following task/section ops, so call it
    /// before them (the desktop invariant). External accounts are fetched live;
    /// a dead account is skipped (its error swallowed), never blanking the list.
    pub fn task_lists_json(&self) -> Result<String, StoreError> {
        let local = self.adapter.list_task_lists_sync().map_err(map_store_err)?;
        for l in &local {
            self.registry.note_task_list_route(&l.id, LOCAL_ID);
        }
        // `list_external_task_lists` stamps external routes internally + swallows
        // per-adapter errors (mirrors `list_external_calendars`).
        let external = self
            .runtime
            .block_on(async { self.registry.list_external_task_lists().await });

        // Snapshot account_id → adapter_kind once so the per-row capability
        // lookup is a cheap map hit (mirrors the desktop). A read failure
        // degrades to "every account looks local" → permissive caps.
        let shared = self.db.shared();
        let account_kinds: std::collections::HashMap<String, AdapterKind> =
            AccountsRepo::new(&shared)
                .list()
                .map(|accounts| {
                    accounts
                        .into_iter()
                        .map(|a| (a.id, a.adapter_kind))
                        .collect()
                })
                .unwrap_or_default();

        // Collect the lists + their per-row metadata, stamp host-local overrides
        // on the lists, then wrap. apply_* no-ops for local lists (own binding,
        // no override row).
        let mut lists: Vec<TaskList> = Vec::with_capacity(local.len() + external.len());
        let mut meta: Vec<(String, TaskCapabilities)> =
            Vec::with_capacity(local.len() + external.len());
        for l in local {
            meta.push((LOCAL_ID.to_string(), local_task_capabilities()));
            lists.push(l);
        }
        for l in external {
            let account_id = self
                .registry
                .account_for_task_list(&l.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            let task_capabilities = self.task_caps_for_account(&account_id, &account_kinds);
            meta.push((account_id, task_capabilities));
            lists.push(l);
        }

        {
            let repo = OverridesRepo::new(&shared);
            apply_to_task_lists(&repo, &mut lists);
            apply_color_to_task_lists(&repo, &mut lists);
        }

        let rows: Vec<TaskListRow> = lists
            .into_iter()
            .zip(meta)
            .map(|(inner, (account_id, task_capabilities))| TaskListRow {
                inner,
                account_id,
                task_capabilities,
            })
            .collect();
        to_json(&rows)
    }

    /// Create a top-level LOCAL task list; returns the created `TaskList` as JSON
    /// and appends `TaskListCreated`. (Always local: this signature carries no
    /// account/parent, so external list creation — which needs both + capability
    /// gating — is a later phase.)
    pub fn create_task_list_json(&self, name: String) -> Result<String, StoreError> {
        let list = self
            .adapter
            .create_task_list(&name, None, None, None, None)
            .map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&list) {
            self.writer.append(SyncEvent::TaskListCreated(EventPayload {
                id: list.id.clone(),
                fields,
            }));
        }
        to_json(&list)
    }

    /// Set or clear a list's parent (`parent_id = None` promotes to top level);
    /// returns the updated `TaskList` as JSON and appends `TaskListUpdated`.
    pub fn reparent_task_list_json(
        &self,
        id: String,
        parent_id: Option<String>,
    ) -> Result<String, StoreError> {
        if !self.is_local_task_list(&id) {
            return Err(external_reparent_unsupported());
        }
        let list = self
            .adapter
            .reparent_task_list(&id, parent_id.as_deref())
            .map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&list) {
            self.writer.append(SyncEvent::TaskListUpdated(EventPayload {
                id: list.id.clone(),
                fields,
            }));
        }
        to_json(&list)
    }

    /// Delete a task list (its tasks cascade away), routed by the list's account.
    /// LOCAL: store delete + `TaskListDeleted`. EXTERNAL: routed to the provider
    /// (which `Unsupported`s unless its manifest allows list deletion) and
    /// self-syncs (no event log).
    pub fn delete_task_list(&self, id: String) -> Result<(), StoreError> {
        match self.route_task_list(&id)? {
            None => {
                self.adapter.delete_task_list(&id).map_err(map_store_err)?;
                self.writer
                    .append(SyncEvent::TaskListDeleted(IdPayload { id }));
                Ok(())
            }
            Some(ext) => self
                .runtime
                .block_on(async { ext.delete_task_list(&id).await })
                .map_err(map_store_err),
        }
    }

    /// Tasks in a list as a JSON array (`cal_core::Task[]`), routed to the list's
    /// owning account (local store or external provider).
    pub fn tasks_json(&self, list_id: String) -> Result<String, StoreError> {
        match self.route_task_list(&list_id)? {
            None => {
                let tasks = self
                    .adapter
                    .get_tasks_sync(&list_id)
                    .map_err(map_store_err)?;
                to_json(&tasks)
            }
            Some(ext) => {
                let tasks = self
                    .runtime
                    .block_on(async { ext.get_tasks(&list_id).await })
                    .map_err(map_store_err)?;
                to_json(&tasks)
            }
        }
    }

    /// One task by id as JSON; [`StoreError::NotFound`] when absent.
    pub fn task_json(&self, id: String) -> Result<String, StoreError> {
        let task = self
            .adapter
            .get_task_by_id(&id)
            .map_err(map_store_err)?
            .ok_or(StoreError::NotFound)?;
        to_json(&task)
    }

    /// Create a task from a JSON `cal_core::NewTask`; returns the created `Task`
    /// as JSON. LOCAL: the store assigns the id (+ a series id for recurring) and
    /// `TaskCreated` is appended. EXTERNAL: routed to the provider, which assigns
    /// its own id and self-syncs (no event log). (The desktop's host-side
    /// series_id assignment + on-demand spawn for external recurring tasks are
    /// deferred — they need the SWR cache the mobile Host doesn't carry.)
    pub fn create_task_json(
        &self,
        list_id: String,
        new_task_json: String,
    ) -> Result<String, StoreError> {
        let new: cal_core::NewTask = from_json("task", &new_task_json)?;
        match self.route_task_list(&list_id)? {
            None => {
                let task = self
                    .adapter
                    .create_task_sync(&list_id, new)
                    .map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&task) {
                    self.writer.append(SyncEvent::TaskCreated(EventPayload {
                        id: task.id.clone(),
                        fields,
                    }));
                }
                to_json(&task)
            }
            Some(ext) => {
                let task = self
                    .runtime
                    .block_on(async { ext.create_task(&list_id, new).await })
                    .map_err(map_store_err)?;
                to_json(&task)
            }
        }
    }

    /// Update a task from a JSON `cal_core::Task`; returns the updated `Task` as
    /// JSON, routed by the task's `list_id`. LOCAL: a single SQL UPDATE (a
    /// list_id change is the desktop's local↔local move) + `TaskUpdated`;
    /// completing a recurring task spawns its next instance locally and the
    /// peer's applier re-runs the spawner deduped on `series_id`, so only
    /// `TaskUpdated` crosses. EXTERNAL: routed to the provider in place (no event
    /// log; it self-syncs). Deferred for external: cross-list moves (needs
    /// previous_list_id, which the mobile signature doesn't carry) + the
    /// on-demand next-instance spawn (cache-dependent) — documented gaps.
    pub fn update_task_json(&self, task_json: String) -> Result<String, StoreError> {
        let task: cal_core::Task = from_json("task", &task_json)?;
        match self.route_task_list(&task.list_id)? {
            None => {
                let updated = self.adapter.update_task_sync(task).map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&updated) {
                    self.writer.append(SyncEvent::TaskUpdated(EventPayload {
                        id: updated.id.clone(),
                        fields,
                    }));
                }
                to_json(&updated)
            }
            Some(ext) => {
                let updated = self
                    .runtime
                    .block_on(async { ext.update_task(task).await })
                    .map_err(map_store_err)?;
                to_json(&updated)
            }
        }
    }

    /// Delete a task, routed by the optional `list_id` (the desktop `delete_task`
    /// shape). LOCAL (or `list_id` omitted → local fallback): delete + append
    /// `TaskDeleted`. EXTERNAL: routed to the provider (no event log). Omitting
    /// `list_id` for an external task can't route — it falls to local and
    /// `NotFound`s; callers that listed the task pass its `list_id`.
    pub fn delete_task(&self, id: String, list_id: Option<String>) -> Result<(), StoreError> {
        let route = match list_id.as_deref() {
            Some(lid) => self.route_task_list(lid)?,
            None => None,
        };
        match route {
            None => {
                self.adapter.delete_task_sync(&id).map_err(map_store_err)?;
                self.writer.append(SyncEvent::TaskDeleted(IdPayload { id }));
                Ok(())
            }
            Some(ext) => self
                .runtime
                .block_on(async { ext.delete_task(&id).await })
                .map_err(map_store_err),
        }
    }

    /// Sections of a list as a JSON array (`cal_core::Section[]`), routed to the
    /// list's owning account.
    pub fn sections_json(&self, list_id: String) -> Result<String, StoreError> {
        match self.route_task_list(&list_id)? {
            None => {
                let sections = self
                    .adapter
                    .list_sections_sync(&list_id)
                    .map_err(map_store_err)?;
                to_json(&sections)
            }
            Some(ext) => {
                let mut sections = self
                    .runtime
                    .block_on(async { ext.list_sections(&list_id).await })
                    .map_err(map_store_err)?;
                // External sections have no provider colour field — stamp any
                // host-local colour overrides. (Local sections carry their own
                // synced binding, handled by the None arm.)
                let shared = self.db.shared();
                let repo = OverridesRepo::new(&shared);
                apply_color_to_sections(&repo, &mut sections);
                to_json(&sections)
            }
        }
    }

    /// Create a section in a list; returns the created `Section` as JSON, routed
    /// by `list_id`. LOCAL: store create (with position + colour) + appends
    /// `SectionCreated`. EXTERNAL: routed to the provider, which takes only
    /// `(list_id, name)` — position + colour are local-only concerns (colour is a
    /// local override) — and self-syncs (no event log).
    pub fn create_section_json(
        &self,
        list_id: String,
        name: String,
        position: u32,
        color_label: Option<String>,
    ) -> Result<String, StoreError> {
        match self.route_task_list(&list_id)? {
            None => {
                let section = self
                    .adapter
                    .create_section(&list_id, &name, position, color_label.map(ColorLabelId))
                    .map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&section) {
                    self.writer.append(SyncEvent::SectionCreated(EventPayload {
                        id: section.id.clone(),
                        fields,
                    }));
                }
                to_json(&section)
            }
            Some(ext) => {
                let section = self
                    .runtime
                    .block_on(async { ext.create_section(&list_id, &name).await })
                    .map_err(map_store_err)?;
                to_json(&section)
            }
        }
    }

    /// Update a section from a JSON `cal_core::Section`; returns it as JSON,
    /// routed by the section's `list_id`. LOCAL: store update + `SectionUpdated`.
    /// EXTERNAL: routed to the provider, which renames only (`list_id`, `id`,
    /// `name`) — colour is a local override, never sent — and self-syncs.
    pub fn update_section_json(&self, section_json: String) -> Result<String, StoreError> {
        let section: cal_core::Section = from_json("section", &section_json)?;
        match self.route_task_list(&section.list_id)? {
            None => {
                let updated = self
                    .adapter
                    .update_section(section)
                    .map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&updated) {
                    self.writer.append(SyncEvent::SectionUpdated(EventPayload {
                        id: updated.id.clone(),
                        fields,
                    }));
                }
                to_json(&updated)
            }
            Some(ext) => {
                let updated = self
                    .runtime
                    .block_on(async {
                        ext.update_section(&section.list_id, &section.id, &section.name)
                            .await
                    })
                    .map_err(map_store_err)?;
                to_json(&updated)
            }
        }
    }

    /// Delete a section (its tasks fall back to ungrouped), routed by the
    /// optional `list_id`. LOCAL (or `list_id` omitted): store delete +
    /// `SectionDeleted`. EXTERNAL: routed to the provider (which needs the
    /// `list_id` to scope the delete) and self-syncs (no event log).
    pub fn delete_section(&self, id: String, list_id: Option<String>) -> Result<(), StoreError> {
        let route = match list_id.as_deref() {
            Some(lid) => self.route_task_list(lid)?,
            None => None,
        };
        match route {
            None => {
                self.adapter.delete_section(&id).map_err(map_store_err)?;
                self.writer
                    .append(SyncEvent::SectionDeleted(IdPayload { id }));
                Ok(())
            }
            Some(ext) => {
                // `route` is Some only when `list_id` was Some.
                let lid = list_id.unwrap_or_default();
                self.runtime
                    .block_on(async { ext.delete_section(&lid, &id).await })
                    .map_err(map_store_err)
            }
        }
    }

    /// The orchestrator's status as JSON (the desktop `SyncStatus` shape:
    /// configured / in_flight / last_synced_at / interval / e2e / …), decorated
    /// with this host's failure latch. The orchestrator always returns
    /// `sustained_failure: false` / `last_error_code: None` (the desktop
    /// scheduler fills those in); here the `SyncProgressDriver` does, so the UI
    /// can warn on a sustained failure. Reads without a sync round.
    pub fn sync_status_json(&self) -> Result<String, StoreError> {
        let mut value = serde_json::to_value(self.orchestrator.status()).map_err(storage_err)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "sustained_failure".to_string(),
                serde_json::Value::Bool(self.progress.sustained()),
            );
            obj.insert(
                "last_error_code".to_string(),
                match self.progress.last_code() {
                    Some(code) => serde_json::Value::String(code),
                    None => serde_json::Value::Null,
                },
            );
            // E2E is transparent above the orchestrator, so report it from the
            // device-local pref (the source of truth the wrap path consults).
            obj.insert(
                "e2e_enabled".to_string(),
                serde_json::Value::Bool(host_core::credential_sync::e2e_enabled(&self.db.shared())),
            );
        }
        Ok(value.to_string())
    }

    /// Enable end-to-end encryption on the configured sync target (§19.7). Mints
    /// fresh v2 key material (a random data key wrapped by a passphrase-derived
    /// KEK, recorded in the plaintext `meta.json`), then branches on the target:
    /// a FRESH target (no `meta.json`) takes the `adopt_local` path (start an
    /// encrypted dataset); an already-populated PLAINTEXT target is RE-ENCRYPTED
    /// in place — every existing log + snapshot is rewritten as ciphertext before
    /// the `meta.json` flip (mirrors the desktop `enable_sync_encryption`).
    /// Either way the data key is stored device-locally and `PREF_E2E_ENABLED`
    /// flips. Other devices must then JOIN with the passphrase
    /// ([`Self::accept_remote_dataset_json`]) or adopt
    /// ([`Self::adopt_remote_encryption_json`]). Returns a JSON report. Rejects an
    /// already-encrypted target + an empty passphrase.
    pub fn enable_sync_encryption_json(&self, passphrase: String) -> Result<String, StoreError> {
        let pp = passphrase.trim();
        if pp.is_empty() {
            return Err(StoreError::InvalidField {
                field: "passphrase".to_string(),
                detail: "passphrase must not be empty".to_string(),
            });
        }
        let shared = self.db.shared();
        if host_core::credential_sync::e2e_enabled(&shared) {
            return Err(StoreError::InvalidField {
                field: "e2e".to_string(),
                detail: "end-to-end encryption is already enabled".to_string(),
            });
        }
        // Rebuild the currently-configured (plaintext) adapter from prefs — E2E
        // is still off here, so restore returns it unwrapped.
        let prefs = UserPrefsRepo::new(&shared);
        let plain = restore_adapter_from_prefs(
            &shared,
            &prefs,
            &self.plugin_manager,
            self.secret_store.as_ref(),
        )
        .ok_or_else(|| StoreError::InvalidField {
            field: "sync".to_string(),
            detail: "configure a sync target before enabling encryption".to_string(),
        })?;
        // Mint v2 material (both paths need it): fresh DEK + a KEK from the
        // passphrase + fresh params; wrap the DEK and record it in the params
        // written to meta.json.
        let mut params = EncryptionParams::fresh();
        let kek = derive_key(pp, &params).map_err(sync_err)?;
        let dek = fresh_data_key();
        let wrapped = wrap_key(&kek, &dek).map_err(sync_err)?;
        params.wrapped_data_key = Some(wrapped);
        // Probe the target to choose the path (also surfaces an unreachable
        // target before we touch any state).
        let meta = self
            .runtime
            .block_on(async { plain.fetch_meta().await })
            .map_err(sync_err)?;
        match meta {
            // A dataset that already declares encryption — another device beat us
            // to it. Joining/adopting is the right path, not re-keying.
            Some(m) if m.e2e_enabled => Err(StoreError::Conflict {
                detail: "this sync target is already encrypted; join it with the \
                         passphrase instead"
                    .to_string(),
            }),
            // FRESH target: adopt a brand-new encrypted dataset. adopt_local
            // writes the E2E meta.json + adopts this device; the pending logs
            // push encrypted on the next round. (No concurrent scheduler on
            // mobile, so the key-after-adopt order is crash-safest: a failure
            // leaves neither key nor flag set.)
            None => {
                let adapter = wrap_if_encrypted(plain, Some(dek));
                let report = self
                    .runtime
                    .block_on(async {
                        self.onboarding
                            .adopt_local(adapter.as_ref(), None, Some(params))
                            .await
                    })
                    .map_err(sync_err)?;
                self.orchestrator.configure(adapter);
                store_e2e_key(self.secret_store.as_ref(), &dek)?;
                prefs
                    .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
                    .map_err(storage_err)?;
                to_json(&report)
            }
            // POPULATED PLAINTEXT target: re-encrypt every existing log + snapshot
            // in place, then flip meta.json (mirrors desktop enable_sync_encryption).
            Some(meta_before) => {
                let encrypting = wrap_if_encrypted(Arc::clone(&plain), Some(dek));
                let mut logs_rewritten = 0usize;
                let mut snapshot_rewritten = false;
                self.runtime
                    .block_on(async {
                        // Fetch each plaintext log via `plain`, push it back
                        // ciphertext via `encrypting` (same path → overwrite).
                        let logs = plain
                            .fetch_new_logs(&sync_core::DeviceCursor::epoch())
                            .await?;
                        for log in &logs {
                            encrypting.push_log(log).await?;
                            logs_rewritten += 1;
                        }
                        if let Some(snapshot) = plain.fetch_snapshot().await? {
                            encrypting.push_snapshot(&snapshot).await?;
                            snapshot_rewritten = true;
                        }
                        Ok::<(), SyncError>(())
                    })
                    .map_err(sync_err)?;
                // Flip local state + swap the orchestrator to encrypting BEFORE
                // publishing the encrypted meta — same ordering rationale as the
                // desktop (a concurrent round must never see "meta encrypted +
                // not-encrypting locally"). Roll back on a meta-push failure.
                store_e2e_key(self.secret_store.as_ref(), &dek)?;
                prefs
                    .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
                    .map_err(storage_err)?;
                self.orchestrator.configure(Arc::clone(&encrypting));
                let mut updated = meta_before;
                updated.e2e_enabled = true;
                updated.e2e_params = Some(params);
                if let Err(err) = self
                    .runtime
                    .block_on(async { plain.push_meta(&updated).await })
                {
                    let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
                    self.orchestrator.configure(plain);
                    return Err(sync_err(err));
                }
                // E2E is on: push pre-encryption local account secrets into the
                // now-encrypted log so the user's other devices pick them up.
                host_core::credential_sync::emit_all_local_credentials(
                    &self.writer,
                    &shared,
                    self.secret_store.as_ref(),
                );
                to_json(&serde_json::json!({
                    "logs_rewritten": logs_rewritten,
                    "snapshot_rewritten": snapshot_rewritten,
                }))
            }
        }
    }

    /// Disable end-to-end encryption on the configured dataset (§19.7) — the
    /// in-place downgrade. Verify `passphrase`, then rewrite every log + snapshot
    /// as PLAINTEXT (decrypting via the data key, stripping the `credential.*`
    /// events/blocks so secrets never reach the now-plaintext storage), flip
    /// `meta.json` to `e2e_enabled = false`, swap the orchestrator to the plain
    /// adapter, and drop the device-local key. Other devices must re-onboard
    /// afterwards (their local state still says encrypted). Mirrors the desktop
    /// `disable_sync_encryption`. Returns `{logs_rewritten, snapshot_rewritten}`.
    pub fn disable_sync_encryption_json(&self, passphrase: String) -> Result<String, StoreError> {
        let pp = passphrase.trim();
        if pp.is_empty() {
            return Err(StoreError::InvalidField {
                field: "passphrase".to_string(),
                detail: "passphrase must not be empty".to_string(),
            });
        }
        // The active adapter is the encrypting one (E2E is on). Reads decrypt
        // through it; meta.json is plaintext either way.
        let encrypting =
            self.orchestrator
                .adapter_handle()
                .ok_or_else(|| StoreError::InvalidField {
                    field: "sync".to_string(),
                    detail: "no sync adapter is configured".to_string(),
                })?;
        let meta_before = self
            .runtime
            .block_on(async { encrypting.fetch_meta().await })
            .map_err(sync_err)?
            .ok_or(StoreError::NotFound)?;
        if !meta_before.e2e_enabled {
            return Err(StoreError::InvalidField {
                field: "e2e".to_string(),
                detail: "this sync target is not encrypted; nothing to disable".to_string(),
            });
        }
        let params = meta_before
            .e2e_params
            .clone()
            .ok_or_else(|| StoreError::Storage {
                detail: "meta.json says e2e but carries no params".to_string(),
            })?;
        // Verify the passphrase + recover the key (wrong passphrase fails here).
        let verified_dek = resolve_data_key(pp, &params).map_err(sync_err)?;
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        // Clear the local E2E flag FIRST: it gates credential emits (so none can
        // leak mid-downgrade) and makes `restore_adapter_from_prefs` rebuild a
        // genuinely PLAIN adapter below. The orchestrator still holds the
        // encrypting adapter, so the flush stays encrypted.
        let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
        // Flush pending logs via the still-encrypting orchestrator so anything
        // queued just before the flag-clear goes up ciphertext (then gets
        // rewritten plaintext below) rather than plaintext after the swap.
        self.runtime
            .block_on(async { self.orchestrator.push_now().await })
            .map_err(sync_err)?;
        // Rebuild the now-genuinely-plain adapter (pref cleared above).
        let plain = restore_adapter_from_prefs(
            &shared,
            &prefs,
            &self.plugin_manager,
            self.secret_store.as_ref(),
        )
        .ok_or_else(|| StoreError::Storage {
            detail: "couldn't rebuild the underlying plain adapter".to_string(),
        })?;
        let mut logs_rewritten = 0usize;
        let mut snapshot_rewritten = false;
        self.runtime
            .block_on(async {
                // Fetch RAW via plain + decrypt ourselves with a plaintext
                // fallback (so a retried/interrupted downgrade is idempotent),
                // strip credential events, push plaintext (same path → overwrite).
                let raw_logs = plain
                    .fetch_new_logs(&sync_core::DeviceCursor::epoch())
                    .await?;
                for raw in raw_logs {
                    let log =
                        host_core::credential_sync::downgrade_log_to_plaintext(&verified_dek, raw);
                    let stripped = host_core::credential_sync::strip_credential_events(&log)?;
                    plain.push_log(&stripped).await?;
                    logs_rewritten += 1;
                }
                // The snapshot can carry account secrets in its credentials
                // block — strip them before the plaintext re-upload.
                if let Some(mut snapshot) = encrypting.fetch_snapshot().await? {
                    host_core::credential_sync::strip_credentials_from_snapshot(&mut snapshot);
                    plain.push_snapshot(&snapshot).await?;
                    snapshot_rewritten = true;
                }
                Ok::<(), SyncError>(())
            })
            .map_err(sync_err)?;
        // Commit the downgrade by overwriting meta.json (plaintext, e2e off).
        let mut updated = meta_before;
        updated.e2e_enabled = false;
        updated.e2e_params = None;
        self.runtime
            .block_on(async { plain.push_meta(&updated).await })
            .map_err(sync_err)?;
        // Swap to the plain adapter + drop the device-local key.
        self.orchestrator.configure(plain);
        delete_e2e_key(self.secret_store.as_ref());
        to_json(&serde_json::json!({
            "logs_rewritten": logs_rewritten,
            "snapshot_rewritten": snapshot_rewritten,
        }))
    }

    /// Probe a sync target WITHOUT committing to it (§19.11 onboarding): build
    /// the adapter from `config_json`, read its `meta.json`, and return a
    /// `SyncPreview` JSON — `{"kind":"empty"}` for a fresh target, or
    /// `{"kind":"existing", e2e_enabled, devices, …}` for one that already holds
    /// a dataset. Side-effect-free (nothing is persisted or activated), so the UI
    /// can offer "join this dataset" vs "start fresh (overwrites)" and let the
    /// user back out. Mirrors the desktop `preview_sync_target`.
    pub fn preview_sync_target_json(&self, config_json: String) -> Result<String, StoreError> {
        let req: ConfigureSyncRequest = from_json("sync config", &config_json)?;
        let adapter = self.build_plain_sync_adapter(&req)?;
        let preview = self
            .runtime
            .block_on(async { self.onboarding.preview(adapter.as_ref()).await })
            .map_err(sync_err)?;
        to_json(&preview)
    }

    /// Join an EXISTING remote dataset (§19.11 "Datensatz übernehmen"): build the
    /// adapter and — when the target is end-to-end encrypted — derive the data
    /// key from `passphrase` + the dataset's `meta.json` params BEFORE pulling
    /// (the applier needs decrypted bytes), wrap the adapter, pull + apply the
    /// remote snapshot + logs, register this device in `meta.json`, then activate
    /// + persist the target (storing the derived E2E key device-locally). This is
    /// how a SECOND device obtains the key for a foreign encrypted dataset —
    /// [`Self::wrap_for_target`] deliberately REFUSES to configure one without it,
    /// so this passphrase-join is the only way in. Mirrors the desktop
    /// `accept_remote_dataset`. Returns the OnboardingReport JSON.
    pub fn accept_remote_dataset_json(
        &self,
        config_json: String,
        device_name: Option<String>,
        passphrase: Option<String>,
    ) -> Result<String, StoreError> {
        let req: ConfigureSyncRequest = from_json("sync config", &config_json)?;
        let plain = self.build_plain_sync_adapter(&req)?;
        self.runtime
            .block_on(async { plain.test_connection().await })
            .map_err(sync_err)?;
        // Peek at meta.json: an encrypted dataset needs its key derived from the
        // passphrase + the dataset's params BEFORE accept_remote reads any
        // snapshot/log (the applier needs plaintext bytes).
        let meta = self
            .runtime
            .block_on(async { plain.fetch_meta().await })
            .map_err(sync_err)?;
        let e2e_active = meta.as_ref().map(|m| m.e2e_enabled).unwrap_or(false);
        let key: Option<[u8; KEY_LEN]> = if e2e_active {
            let pp = passphrase
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| StoreError::Auth {
                    detail: "this dataset is encrypted; a passphrase is required".to_string(),
                })?;
            let params = meta
                .as_ref()
                .and_then(|m| m.e2e_params.clone())
                .ok_or_else(|| StoreError::Storage {
                    detail: "meta.json says e2e but carries no params".to_string(),
                })?;
            // resolve_data_key handles both v1 (the passphrase IS the DEK) and v2
            // (a passphrase-derived KEK unwraps the stored DEK) layouts; a wrong
            // passphrase surfaces here as an Auth error.
            Some(resolve_data_key(pp, &params).map_err(sync_err)?)
        } else {
            None
        };
        let adapter = wrap_if_encrypted(plain, key);
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        // Flip the local E2E flag BEFORE applying: the snapshot's credential
        // restore is gated on PREF_E2E_ENABLED (never write synced secrets on a
        // plaintext-mode device), so joining an E2E dataset must set it first or
        // every account's password is silently dropped. Reverted on failure.
        if e2e_active {
            prefs
                .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
                .map_err(storage_err)?;
        }
        let trimmed = device_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let report = match self.runtime.block_on(async {
            self.onboarding
                .accept_remote(adapter.as_ref(), trimmed)
                .await
        }) {
            Ok(report) => report,
            Err(err) => {
                if e2e_active {
                    let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
                }
                return Err(sync_err(err));
            }
        };
        // Commit the rest of the choice now that onboarding has succeeded.
        self.orchestrator.configure(adapter);
        self.persist_sync_config(&req)?;
        if let Some(k) = key {
            store_e2e_key(self.secret_store.as_ref(), &k)?;
        } else {
            // Joining a plaintext dataset clears any stale flag.
            let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
        }
        to_json(&report)
    }

    /// Rotate the dataset's E2E passphrase (§19.7): verify `old_passphrase`
    /// against the dataset's `meta.json` params (recovering the UNCHANGED data
    /// key), then mint a fresh KEK from `new_passphrase` over a freshly-rotated
    /// salt and re-wrap the same data key, pushing the updated `meta.json`. The
    /// data key never changes, so every already-onboarded device keeps working
    /// (its keychain DEK is untouched) — only devices that JOIN from here on need
    /// the new passphrase. The `meta.json` push is the single committing step.
    /// Mirrors the desktop `change_sync_passphrase`, incl. the silent v1→v2
    /// migration (the re-wrap writes `wrapped_data_key` even on a legacy dataset
    /// that lacked it).
    pub fn change_sync_passphrase_json(
        &self,
        old_passphrase: String,
        new_passphrase: String,
    ) -> Result<(), StoreError> {
        let new_pp = new_passphrase.trim();
        if new_pp.is_empty() {
            return Err(StoreError::InvalidField {
                field: "new_passphrase".to_string(),
                detail: "new passphrase must not be empty".to_string(),
            });
        }
        let old_pp = old_passphrase.trim();
        if old_pp.is_empty() {
            return Err(StoreError::InvalidField {
                field: "old_passphrase".to_string(),
                detail: "current passphrase must not be empty".to_string(),
            });
        }
        let adapter =
            self.orchestrator
                .adapter_handle()
                .ok_or_else(|| StoreError::InvalidField {
                    field: "sync".to_string(),
                    detail: "no sync adapter is configured".to_string(),
                })?;
        // meta.json is always plaintext (§19.7), so fetch_meta passes through the
        // encrypting wrapper unchanged.
        let meta = self
            .runtime
            .block_on(async { adapter.fetch_meta().await })
            .map_err(sync_err)?
            .ok_or(StoreError::NotFound)?;
        if !meta.e2e_enabled {
            return Err(StoreError::InvalidField {
                field: "e2e".to_string(),
                detail: "this sync target is not encrypted; nothing to rotate".to_string(),
            });
        }
        let current_params = meta.e2e_params.clone().ok_or_else(|| StoreError::Storage {
            detail: "meta.json says e2e but carries no params".to_string(),
        })?;
        // Verify the old passphrase + recover the data key (v1: the passphrase
        // IS the key; v2: a KEK that unwraps the stored DEK). A wrong passphrase
        // fails here, before anything is written.
        let dek = resolve_data_key(old_pp, &current_params).map_err(sync_err)?;
        // Defence in depth: re-assert the recovered DEK in the keychain so the
        // next boot loads the right key even if the slot had drifted.
        store_e2e_key(self.secret_store.as_ref(), &dek)?;
        // Fresh salt → a precomputed table against the old wrap is worthless;
        // re-wrap the SAME DEK with the new-passphrase KEK.
        let mut new_params = current_params;
        new_params.rotate_salt();
        new_params.wrapped_data_key = None;
        let new_kek = derive_key(new_pp, &new_params).map_err(sync_err)?;
        let new_wrap = wrap_key(&new_kek, &dek).map_err(sync_err)?;
        new_params.wrapped_data_key = Some(new_wrap);
        let mut updated = meta;
        updated.e2e_params = Some(new_params);
        // The single committing step: once this lands, the new passphrase is
        // authoritative for future joins.
        self.runtime
            .block_on(async { adapter.push_meta(&updated).await })
            .map_err(sync_err)?;
        Ok(())
    }

    /// Adopt encryption a PEER turned on (§19.7): this device was syncing the
    /// dataset in PLAINTEXT, a peer enabled E2E, and the next round failed with
    /// `encryption_required` (the orchestrator's encryption gate). Pure unlock —
    /// derive the dataset's data key from `passphrase` + the `meta.json` params,
    /// swap the orchestrator onto an encrypting adapter, flip the local E2E pref,
    /// store the key device-locally, and re-emit any pre-encryption account
    /// secrets into the now-encrypted log so the other devices pick them up. No
    /// re-encryption / device registration (the enabling device already did
    /// those). After this, the next `sync_now` passes the gate and applies the
    /// dataset decrypted. Mirrors the desktop `adopt_remote_encryption`.
    pub fn adopt_remote_encryption_json(&self, passphrase: String) -> Result<(), StoreError> {
        let pp = passphrase.trim();
        if pp.is_empty() {
            return Err(StoreError::InvalidField {
                field: "passphrase".to_string(),
                detail: "passphrase must not be empty".to_string(),
            });
        }
        // The configured adapter is plain (we're here precisely because local
        // e2e is off). meta.json is always plaintext, so fetch_meta passes
        // through unchanged.
        let plain = self
            .orchestrator
            .adapter_handle()
            .ok_or_else(|| StoreError::InvalidField {
                field: "sync".to_string(),
                detail: "no sync adapter is configured".to_string(),
            })?;
        let meta = self
            .runtime
            .block_on(async { plain.fetch_meta().await })
            .map_err(sync_err)?
            .ok_or(StoreError::NotFound)?;
        if !meta.e2e_enabled {
            return Err(StoreError::InvalidField {
                field: "e2e".to_string(),
                detail: "this sync target is not encrypted; nothing to adopt".to_string(),
            });
        }
        let params = meta.e2e_params.clone().ok_or_else(|| StoreError::Storage {
            detail: "meta.json says e2e but carries no params".to_string(),
        })?;
        // Verify the passphrase + recover the dataset key (a wrong passphrase
        // fails here, before any state changes).
        let dek = resolve_data_key(pp, &params).map_err(sync_err)?;
        let encrypting = wrap_if_encrypted(plain, Some(dek));
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        // Store the key, flip the pref, then swap the adapter — the same order
        // as the desktop so the next-boot restore wraps to match.
        store_e2e_key(self.secret_store.as_ref(), &dek)?;
        prefs
            .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
            .map_err(storage_err)?;
        self.orchestrator.configure(encrypting);
        // E2E is now on for this device too. Push any local account secret that
        // predates the encryption (created while syncing in plaintext, so it
        // never got a `credential.set`) into the now-encrypted log so those
        // accounts reach the other devices without re-entry. Idempotent + gated
        // on the E2E pref we just set.
        host_core::credential_sync::emit_all_local_credentials(
            &self.writer,
            &shared,
            self.secret_store.as_ref(),
        );
        Ok(())
    }

    /// Complete a host-driven OAuth flow for a SYNC adapter (`plugin_id` =
    /// `com.aperio.sync-adapter-dropbox` / `…-googledrive`): exchange the
    /// redirect's `code` (+ the `pkce_verifier`/`state` from
    /// [`Self::begin_oauth_json`]) for tokens via the plugin (`phase:"exchange"`,
    /// the network step + CSRF check), then store the refresh token in the
    /// adapter's keychain slot (a fixed pseudo-account, one per kind — NOT an
    /// account row; sync credentials are managed independently). Unlike the
    /// account OAuth this creates NO account + appends NO event; the caller
    /// follows with [`Self::configure_sync_adapter_json`] to activate the target.
    /// Mirrors the desktop `connect_dropbox_oauth` / `connect_googledrive_oauth`.
    pub fn complete_sync_oauth_json(
        &self,
        plugin_id: String,
        request_json: String,
    ) -> Result<(), StoreError> {
        let secret_account = match plugin_id.as_str() {
            PLUGIN_ID_DROPBOX => DROPBOX_SECRET_ACCOUNT,
            PLUGIN_ID_GOOGLEDRIVE => GOOGLEDRIVE_SECRET_ACCOUNT,
            other => {
                return Err(StoreError::InvalidField {
                    field: "plugin_id".to_string(),
                    detail: format!("'{other}' is not an OAuth sync adapter"),
                });
            }
        };
        let req: CompleteSyncOAuthRequest = from_json("sync oauth complete", &request_json)?;
        let exchange_args = serde_json::json!({
            "phase": "exchange",
            "client_id": req.client_id,
            "client_secret": req.client_secret,
            "code": req.code,
            "pkce_verifier": req.pkce_verifier,
            "state": req.state,
            "returned_state": req.returned_state,
            "redirect_uri": req.redirect_uri,
        });
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .interactive_auth(&plugin_id, &exchange_args.to_string())
                    .await
            })
            .map_err(|e| StoreError::Auth {
                detail: e.to_string(),
            })?;
        let tokens: OAuthTokenJson =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Protocol {
                detail: format!("token blob: {e}"),
            })?;
        // The sync adapter re-mints access tokens from the refresh token, so the
        // refresh token is the credential we persist; its absence means offline
        // access wasn't granted and the target can't be kept alive.
        let refresh = tokens.refresh_token.ok_or_else(|| StoreError::Protocol {
            detail: "the provider returned no refresh token (offline access not granted)"
                .to_string(),
        })?;
        self.secret_store
            .store(secret_account, SecretSlot::RefreshToken, &refresh)
            .map_err(|e| StoreError::Storage {
                detail: format!("store refresh token: {e}"),
            })?;
        Ok(())
    }

    /// Probe an SFTP server's SHA256 host-key fingerprint WITHOUT pinning it, and
    /// classify it against the device's pin store (§19.5 TOFU). `args_json`
    /// carries `{host, port}`; returns `{host_port, fingerprint, status}` where
    /// `status` is `{kind:"new"}` (nothing pinned) / `{kind:"unchanged"}` /
    /// `{kind:"changed", stored}`. The UI shows the trust dialog accordingly,
    /// then calls [`Self::trust_sftp_host_key`] before configuring. Mirrors the
    /// desktop `preview_sftp_host_key`; the SSH probe is verified on-device.
    pub fn preview_sftp_host_key_json(&self, args_json: String) -> Result<String, StoreError> {
        #[derive(serde::Deserialize)]
        struct PreviewArgs {
            host: String,
            #[serde(default = "default_ssh_port")]
            port: u16,
        }
        fn default_ssh_port() -> u16 {
            22
        }
        let args: PreviewArgs = from_json("sftp preview", &args_json)?;
        let host = args.host.trim();
        if host.is_empty() {
            return Err(StoreError::InvalidField {
                field: "host".to_string(),
                detail: "SFTP host must not be empty".to_string(),
            });
        }
        let probe_args = serde_json::json!({ "host": host, "port": args.port });
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .probe_host_key(PLUGIN_ID_SFTP, &probe_args.to_string())
                    .await
            })
            .map_err(map_probe_err)?;
        #[derive(serde::Deserialize)]
        struct ProbeResult {
            fingerprint: String,
        }
        let probe: ProbeResult =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Protocol {
                detail: format!("host-key probe blob: {e}"),
            })?;
        let host_port = format!("{host}:{}", args.port);
        let status = match UserPrefsHostKeyVerifier::new(self.db.shared()).peek(&host_port) {
            None => serde_json::json!({ "kind": "new" }),
            Some(ref s) if *s == probe.fingerprint => serde_json::json!({ "kind": "unchanged" }),
            Some(s) => serde_json::json!({ "kind": "changed", "stored": s }),
        };
        Ok(serde_json::json!({
            "host_port": host_port,
            "fingerprint": probe.fingerprint,
            "status": status,
        })
        .to_string())
    }

    /// Pin a user-confirmed SFTP host-key fingerprint for `host_port` (§19.5 —
    /// always an explicit user gesture, for first-use AND key-change). The UI
    /// calls this after the trust dialog, then configures. Mirrors the desktop
    /// `trust_sftp_host_key`.
    pub fn trust_sftp_host_key(
        &self,
        host_port: String,
        fingerprint: String,
    ) -> Result<(), StoreError> {
        let host_port = host_port.trim();
        let fingerprint = fingerprint.trim();
        if host_port.is_empty() {
            return Err(StoreError::InvalidField {
                field: "host_port".to_string(),
                detail: "host_port must not be empty".to_string(),
            });
        }
        if fingerprint.is_empty() {
            return Err(StoreError::InvalidField {
                field: "fingerprint".to_string(),
                detail: "fingerprint must not be empty".to_string(),
            });
        }
        UserPrefsHostKeyVerifier::new(self.db.shared()).record(host_port, fingerprint);
        Ok(())
    }

    /// Drop the pinned SFTP fingerprint for `host_port` (the "forget pin"
    /// gesture; the next connect re-runs the first-use trust dialog).
    pub fn forget_sftp_host_key(&self, host_port: String) -> Result<(), StoreError> {
        let host_port = host_port.trim();
        if host_port.is_empty() {
            return Err(StoreError::InvalidField {
                field: "host_port".to_string(),
                detail: "host_port must not be empty".to_string(),
            });
        }
        UserPrefsHostKeyVerifier::new(self.db.shared()).forget(host_port);
        Ok(())
    }

    /// The currently-pinned SFTP fingerprint for `host_port`, or `None`. Lets the
    /// UI show "Pinned: SHA256:…" + the forget gesture without probing the server.
    pub fn pinned_sftp_host_key(&self, host_port: String) -> Result<Option<String>, StoreError> {
        let host_port = host_port.trim();
        if host_port.is_empty() {
            return Ok(None);
        }
        Ok(UserPrefsHostKeyVerifier::new(self.db.shared()).peek(host_port))
    }

    /// Configure the sync adapter from a JSON request (`local`/`webdav`/`ftp`/
    /// `dropbox`/`googledrive`/`sftp`): build the plain adapter via
    /// [`Self::build_plain_sync_adapter`], probe it (`test_connection`), apply
    /// the E2E gate ([`Self::wrap_for_target`] — wrap an encrypted target with
    /// the device-local key or refuse), make it the orchestrator's active
    /// adapter, then persist the choice ([`Self::persist_sync_config`] — the
    /// `sync.adapter.*` prefs are device-local; the is_synced_key allowlist
    /// excludes them, so they never propagate; secrets go to the keychain). This
    /// is the "start fresh / overwrite" path; joining an existing dataset is
    /// [`Self::accept_remote_dataset_json`].
    pub fn configure_sync_adapter_json(&self, config_json: String) -> Result<(), StoreError> {
        let req: ConfigureSyncRequest = from_json("sync config", &config_json)?;
        let adapter = self.build_plain_sync_adapter(&req)?;
        // Probe before keeping it active so a bad path/creds/URL fails here
        // rather than on the first silent sync round.
        self.runtime
            .block_on(async { adapter.test_connection().await })
            .map_err(sync_err)?;
        // E2E gate: wrap (or refuse) per the target's meta before activating.
        let adapter = self.wrap_for_target(adapter)?;
        self.orchestrator.configure(adapter);
        self.persist_sync_config(&req)?;
        Ok(())
    }

    /// Run one sync round (push local pending logs, fetch + apply foreign ones,
    /// compaction audit) and return the `SyncRoundReport` as JSON. Errors with
    /// "not configured" until `configure_sync_adapter_json` has run. Records the
    /// round's outcome in the failure latch (success resets it).
    pub fn sync_now_json(&self) -> Result<String, StoreError> {
        match self
            .runtime
            .block_on(async { self.orchestrator.sync_now().await })
        {
            Ok(report) => {
                self.progress.record_success();
                to_json(&report)
            }
            Err(e) => {
                self.progress.record_failure(e.code());
                Err(sync_err(e))
            }
        }
    }

    /// Push the local pending logs without fetching (call from RN AppState
    /// "background"). Returns the number of logs pushed. Records the outcome in
    /// the failure latch like `sync_now`.
    pub fn push_now(&self) -> Result<u32, StoreError> {
        match self
            .runtime
            .block_on(async { self.orchestrator.push_now().await })
        {
            Ok(pushed) => {
                self.progress.record_success();
                Ok(pushed as u32)
            }
            Err(e) => {
                self.progress.record_failure(e.code());
                Err(sync_err(e))
            }
        }
    }

    /// Upcoming reminder triggers within `horizon_minutes` from now, as a JSON
    /// array of `{item_id, item_kind, title, body, trigger_at}` sorted ascending
    /// by trigger time, for the mobile layer to register as ahead-of-time OS
    /// local notifications. Combines local + external sources through the SAME
    /// `host_core::reminders` enumeration the desktop scheduler uses (one source
    /// of truth for what fires when). Only future triggers are returned (a past
    /// notification can't be scheduled); duplicates (an item surfacing from both
    /// local SQLite and an external adapter) are collapsed, matching the desktop
    /// dedup key `(item_id, trigger_at)`.
    pub fn upcoming_reminders_json(&self, horizon_minutes: u32) -> Result<String, StoreError> {
        let now = chrono::Utc::now();
        let latest = now + chrono::Duration::minutes(i64::from(horizon_minutes));
        let shared = self.db.shared();
        let mut triggers = self.runtime.block_on(async {
            host_core::reminders::enumerate_triggers(&shared, &self.registry, now, latest).await
        });
        triggers.sort_by_key(|t| t.trigger_at);
        let mut seen = std::collections::HashSet::new();
        let dtos: Vec<serde_json::Value> = triggers
            .into_iter()
            // Strictly future: a trigger at exactly `now` can't be scheduled as a
            // future OS notification (and `enumerate_triggers` includes the
            // `>= now` boundary), so drop it — honouring the "future-only"
            // contract + matching the JS scheduler's `> now` gate. Dedup after,
            // so an excluded boundary trigger doesn't consume a `seen` slot.
            .filter(|t| t.trigger_at > now && seen.insert((t.item_id.clone(), t.trigger_at)))
            .map(|t| {
                serde_json::json!({
                    "item_id": t.item_id,
                    "item_kind": t.item_kind,
                    "title": t.title,
                    "body": t.body,
                    "trigger_at": t.trigger_at.to_rfc3339(),
                })
            })
            .collect();
        to_json(&dtos)
    }

    // ── User preferences (generic key/value; synced-key whitelist) ────────────
    //
    // Opaque string values (the JS layer serialises JSON for structured prefs).
    // A write/delete against a §19.2.1 whitelisted key (locale, week-start,
    // appearance, sound config, default reminders, …) appends a `SettingsUpdated`
    // event so the change syncs to the user's other devices; local-only keys
    // (sidebar state, device id, …) don't. Mirrors the desktop user_prefs
    // commands; the substrate for the synced settings panels.

    /// Read a user preference, or `None` when unset.
    pub fn get_user_pref(&self, key: String) -> Result<Option<String>, StoreError> {
        let shared = self.db.shared();
        UserPrefsRepo::new(&shared).get(&key).map_err(storage_err)
    }

    /// Upsert a user preference. A whitelisted key also appends `SettingsUpdated`
    /// (wire value = the stored string parsed as JSON, else wrapped as a JSON
    /// string — same round-trip as the desktop).
    pub fn set_user_pref(&self, key: String, value: String) -> Result<(), StoreError> {
        let shared = self.db.shared();
        UserPrefsRepo::new(&shared)
            .set(&key, &value)
            .map_err(storage_err)?;
        if sync_engine::whitelist::is_synced_key(&key) {
            let payload_value = serde_json::from_str(&value)
                .unwrap_or_else(|_| serde_json::Value::String(value.clone()));
            self.writer
                .append(SyncEvent::SettingsUpdated(SettingsPayload {
                    key,
                    value: payload_value,
                }));
        }
        Ok(())
    }

    /// Delete a user preference. A whitelisted key appends `SettingsUpdated` with
    /// a null value (the applier reads null as "remove the row"), keeping the
    /// wire shape uniform with set.
    pub fn delete_user_pref(&self, key: String) -> Result<(), StoreError> {
        let shared = self.db.shared();
        UserPrefsRepo::new(&shared)
            .delete(&key)
            .map_err(storage_err)?;
        if sync_engine::whitelist::is_synced_key(&key) {
            self.writer
                .append(SyncEvent::SettingsUpdated(SettingsPayload {
                    key,
                    value: serde_json::Value::Null,
                }));
        }
        Ok(())
    }

    // ── Colour labels (app-wide palette; local-only, always synced) ───────────
    //
    // Colour labels live ONLY in local SQLite (§8 — no external provider has the
    // concept), so every mutation is LOCAL and unconditionally appends a sync
    // event (no account routing). Mirrors the desktop color_labels commands.

    /// All colour labels (named + ad-hoc) as a JSON `ColorLabel[]`.
    pub fn list_color_labels_json(&self) -> Result<String, StoreError> {
        let labels = self.adapter.list_color_labels().map_err(map_store_err)?;
        to_json(&labels)
    }

    /// Create a named colour label from `{name, hex}`; returns the created
    /// `ColorLabel` as JSON and appends `ColorLabelCreated`.
    pub fn create_color_label_json(&self, name: String, hex: String) -> Result<String, StoreError> {
        let label = self
            .adapter
            .create_color_label(&name, &hex)
            .map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&label) {
            self.writer
                .append(SyncEvent::ColorLabelCreated(EventPayload {
                    id: label.id.as_str().to_string(),
                    fields,
                }));
        }
        to_json(&label)
    }

    /// Resolve a one-off `hex` to a hidden ad-hoc colour label (dedup by hex),
    /// creating one when needed; appends `ColorLabelCreated` only on a genuine
    /// create (re-picking the same colour doesn't spam the log). The custom
    /// colour picker calls this; named colours go through `create_color_label`.
    pub fn get_or_create_ad_hoc_color_label_json(&self, hex: String) -> Result<String, StoreError> {
        let (label, created) = self
            .adapter
            .get_or_create_ad_hoc_color_label(&hex)
            .map_err(map_store_err)?;
        if created {
            if let Ok(fields) = serde_json::to_value(&label) {
                self.writer
                    .append(SyncEvent::ColorLabelCreated(EventPayload {
                        id: label.id.as_str().to_string(),
                        fields,
                    }));
            }
        }
        to_json(&label)
    }

    /// Update a colour label from a JSON `ColorLabel`; returns it and appends
    /// `ColorLabelUpdated`.
    pub fn update_color_label_json(&self, label_json: String) -> Result<String, StoreError> {
        let label: cal_core::ColorLabel = from_json("color label", &label_json)?;
        let updated = self
            .adapter
            .update_color_label(label)
            .map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&updated) {
            self.writer
                .append(SyncEvent::ColorLabelUpdated(EventPayload {
                    id: updated.id.as_str().to_string(),
                    fields,
                }));
        }
        to_json(&updated)
    }

    /// Delete a colour label by id; appends `ColorLabelDeleted`. (Entities still
    /// referencing it resolve to no colour, matching the desktop.)
    pub fn delete_color_label(&self, id: String) -> Result<(), StoreError> {
        self.adapter
            .delete_color_label(&id)
            .map_err(map_store_err)?;
        self.writer
            .append(SyncEvent::ColorLabelDeleted(IdPayload { id }));
        Ok(())
    }

    /// Set or clear a container's bound colour label (DESIGN §8.2). Mirrors the
    /// desktop `set_container_color_label`: a LOCAL calendar / task list carries
    /// the binding on its own (synced) row (update + emit the matching sync
    /// event); an EXTERNAL container — and EVERY contact list, even local ones —
    /// stores a host-local `OverridesRepo` colour override (the read paths stamp
    /// it back on). `kind` is `"calendar"` | `"task_list"` | `"contact_list"`;
    /// `color_label_id` `None` clears it.
    pub fn set_container_color_label(
        &self,
        container_id: String,
        kind: String,
        color_label_id: Option<String>,
    ) -> Result<(), StoreError> {
        match kind.as_str() {
            "calendar" if self.is_local_calendar(&container_id) => {
                if let Some(mut cal) = self
                    .adapter
                    .get_calendar_by_id(&container_id)
                    .map_err(map_store_err)?
                {
                    cal.color_label = color_label_id.map(ColorLabelId);
                    let updated = self.adapter.update_calendar(cal).map_err(map_store_err)?;
                    if let Ok(fields) = serde_json::to_value(&updated) {
                        self.writer.append(SyncEvent::CalendarUpdated(EventPayload {
                            id: updated.id.clone(),
                            fields,
                        }));
                    }
                }
                Ok(())
            }
            "task_list" if self.is_local_task_list(&container_id) => {
                if let Some(mut list) = self
                    .adapter
                    .get_task_list_by_id(&container_id)
                    .map_err(map_store_err)?
                {
                    list.color_label = color_label_id.map(ColorLabelId);
                    let updated = self.adapter.update_task_list(list).map_err(map_store_err)?;
                    if let Ok(fields) = serde_json::to_value(&updated) {
                        self.writer.append(SyncEvent::TaskListUpdated(EventPayload {
                            id: updated.id.clone(),
                            fields,
                        }));
                    }
                }
                Ok(())
            }
            // External calendar / task list, or any contact list → host-local
            // colour override (the read paths stamp it back).
            _ => {
                let ck = parse_container_kind(&kind)?;
                let shared = self.db.shared();
                let repo = OverridesRepo::new(&shared);
                match color_label_id {
                    Some(id) => repo
                        .set_color_label(&container_id, ck, &id)
                        .map_err(map_overrides_err)?,
                    None => repo
                        .clear_color_label(&container_id, ck)
                        .map_err(map_overrides_err)?,
                }
                Ok(())
            }
        }
    }

    /// Rename a container (DESIGN §6.5). Mirrors the desktop `set_container_name`:
    /// a LOCAL calendar / task list is renamed on its own (synced) row (+ emits
    /// the sync event); an EXTERNAL container's rename is pushed to its provider
    /// first and, only if the provider declares it `Unsupported`, falls back to a
    /// host-local name override (cleared on a successful provider rename so the
    /// source name stays the single truth). A contact list has no source-rename
    /// path, so it always lands as an override. `kind` is `"calendar"` |
    /// `"task_list"` | `"contact_list"`.
    pub fn rename_container(
        &self,
        container_id: String,
        kind: String,
        name: String,
    ) -> Result<(), StoreError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidField {
                field: "name".to_string(),
                detail: "name must not be empty".to_string(),
            });
        }
        match kind.as_str() {
            "calendar" if self.is_local_calendar(&container_id) => {
                if let Some(mut cal) = self
                    .adapter
                    .get_calendar_by_id(&container_id)
                    .map_err(map_store_err)?
                {
                    cal.name = trimmed.to_string();
                    let updated = self.adapter.update_calendar(cal).map_err(map_store_err)?;
                    if let Ok(fields) = serde_json::to_value(&updated) {
                        self.writer.append(SyncEvent::CalendarUpdated(EventPayload {
                            id: updated.id.clone(),
                            fields,
                        }));
                    }
                }
                Ok(())
            }
            "task_list" if self.is_local_task_list(&container_id) => {
                if let Some(mut list) = self
                    .adapter
                    .get_task_list_by_id(&container_id)
                    .map_err(map_store_err)?
                {
                    list.name = trimmed.to_string();
                    let updated = self.adapter.update_task_list(list).map_err(map_store_err)?;
                    if let Ok(fields) = serde_json::to_value(&updated) {
                        self.writer.append(SyncEvent::TaskListUpdated(EventPayload {
                            id: updated.id.clone(),
                            fields,
                        }));
                    }
                }
                Ok(())
            }
            // External container (or a contact list): push to the provider first,
            // override only on Unsupported.
            _ => {
                let ck = parse_container_kind(&kind)?;
                // Address books have no provider rename path AND the name-override
                // table excludes them (CHECK kind IN ('calendar','task_list')), so
                // a contact-list rename is genuinely unsupported.
                if matches!(ck, ContainerKind::ContactList) {
                    return Err(StoreError::Unsupported {
                        detail: "renaming address books is not supported".to_string(),
                    });
                }
                let account = match ck {
                    ContainerKind::Calendar => self.registry.account_for_calendar(&container_id),
                    ContainerKind::TaskList => self.registry.account_for_task_list(&container_id),
                    ContainerKind::ContactList => {
                        self.registry.account_for_contact_list(&container_id)
                    }
                }
                .unwrap_or_else(|| LOCAL_ID.to_string());
                let push_result: cal_core::Result<()> = self.runtime.block_on(async {
                    match ck {
                        ContainerKind::Calendar => match self.registry.calendar_adapter(&account) {
                            Some(ext) => ext.rename_calendar(&container_id, trimmed).await,
                            None => Err(cal_core::Error::NotFound(format!(
                                "no adapter registered for account '{account}'"
                            ))),
                        },
                        ContainerKind::TaskList => match self.registry.task_adapter(&account) {
                            Some(ext) => ext.rename_task_list(&container_id, trimmed).await,
                            None => Err(cal_core::Error::NotFound(format!(
                                "no adapter registered for account '{account}'"
                            ))),
                        },
                        // Address books have no source-rename path.
                        ContainerKind::ContactList => Err(cal_core::Error::Unsupported(
                            "renaming address books is not supported".to_string(),
                        )),
                    }
                });
                let shared = self.db.shared();
                let repo = OverridesRepo::new(&shared);
                match push_result {
                    // Source accepted it — drop any stale override (non-fatal).
                    Ok(()) => {
                        let _ = repo.clear(&container_id, ck);
                        Ok(())
                    }
                    // Read-only source — the new name can only live as an override.
                    Err(cal_core::Error::Unsupported(_)) => {
                        repo.set(&container_id, ck, trimmed)
                            .map_err(map_overrides_err)?;
                        Ok(())
                    }
                    Err(other) => Err(map_store_err(other)),
                }
            }
        }
    }

    /// Set or clear a SECTION's colour label (DESIGN §8.2). Routed by the owning
    /// list's account: a LOCAL section carries the binding on its own (synced)
    /// row (+ `SectionUpdated`); an EXTERNAL section (Todoist / Vikunja — no
    /// provider colour field) stores a host-local `OverridesRepo` override.
    pub fn set_section_color(
        &self,
        section_id: String,
        list_id: String,
        color_label_id: Option<String>,
    ) -> Result<(), StoreError> {
        if self.is_local_task_list(&list_id) {
            if let Some(mut section) = self
                .adapter
                .get_section_by_id(&section_id)
                .map_err(map_store_err)?
            {
                section.color_label = color_label_id.map(ColorLabelId);
                let updated = self
                    .adapter
                    .update_section(section)
                    .map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&updated) {
                    self.writer.append(SyncEvent::SectionUpdated(EventPayload {
                        id: updated.id.clone(),
                        fields,
                    }));
                }
            }
            return Ok(());
        }
        let shared = self.db.shared();
        let repo = OverridesRepo::new(&shared);
        match color_label_id {
            Some(id) => repo
                .set_section_color_label(&section_id, &id)
                .map_err(map_overrides_err)?,
            None => repo
                .clear_section_color_label(&section_id)
                .map_err(map_overrides_err)?,
        }
        Ok(())
    }

    /// Set or clear an EVENT's colour override (DESIGN §8.2). A LOCAL event — and
    /// a colour-capable external calendar — carry the colour natively through
    /// `update_event` (the frontend routes those there), so this is a no-op for
    /// local; a non-colour-capable EXTERNAL event stores a host-local
    /// `OverridesRepo` override. `event_id` is the series master id. (Mobile has
    /// no read cache to check `supports_event_color`, so it gates on locality
    /// only — `apply_color_to_events` skips events carrying a native colour, so a
    /// stray override can never shadow a provider colour.)
    pub fn set_event_color(
        &self,
        event_id: String,
        calendar_id: String,
        color_label_id: Option<String>,
    ) -> Result<(), StoreError> {
        if self.is_local_calendar(&calendar_id) {
            return Ok(());
        }
        let shared = self.db.shared();
        let repo = OverridesRepo::new(&shared);
        match color_label_id {
            Some(id) => repo
                .set_event_color_label(&event_id, &id)
                .map_err(map_overrides_err)?,
            None => repo
                .clear_event_color_label(&event_id)
                .map_err(map_overrides_err)?,
        }
        Ok(())
    }

    // ── Contacts (JSON bridge, routed) ────────────────────────────────────────
    //
    // Address books + contacts, routed local (the device SQLite store) vs
    // external (CardDAV / Google / EWS providers) by `route_contact_list`.
    // Contacts are NOT on the sync event log (no `Contact*` SyncEvent) — local
    // contacts are device-local; external ones self-sync via their provider — so
    // no `writer.append` here (unlike tasks/events). All the read/write adapter
    // methods are async `ContactsFeature`, driven via `block_on`; contact-LIST
    // create/delete are local-only inherent (sync) methods. The JSON wire is the
    // `cal_core` serde shape. (Photo bytes + search + the rich address/member
    // fields are a later phase; the first mobile screen covers name/emails/
    // phones/org.)

    /// All contact lists (local + external) as a JSON `ContactListRow[]` (each
    /// `ContactList` flattened + its `account_id`). Fetches external live (errors
    /// swallowed per-adapter) + primes the list→account route map for the
    /// following contact ops — call it first.
    pub fn contact_lists_json(&self) -> Result<String, StoreError> {
        let (local, external) = self.runtime.block_on(async {
            let local = self.adapter.list_contact_lists().await;
            let external = self.registry.list_external_contact_lists().await;
            (local, external)
        });
        let local = local.map_err(map_store_err)?;
        for l in &local {
            self.registry.note_contact_list_route(&l.id, LOCAL_ID);
        }
        // Collect the lists + accounts, stamp host-local colour overrides, then
        // wrap. Contact lists bind colour ONLY via overrides (even local ones —
        // they aren't event-log-synced), so this applies to every row.
        let mut lists: Vec<ContactList> = Vec::with_capacity(local.len() + external.len());
        let mut accounts: Vec<String> = Vec::with_capacity(local.len() + external.len());
        for l in local {
            accounts.push(LOCAL_ID.to_string());
            lists.push(l);
        }
        for l in external {
            accounts.push(
                self.registry
                    .account_for_contact_list(&l.id)
                    .unwrap_or_else(|| LOCAL_ID.to_string()),
            );
            lists.push(l);
        }
        {
            let shared = self.db.shared();
            let repo = OverridesRepo::new(&shared);
            apply_color_to_contact_lists(&repo, &mut lists);
        }
        let rows: Vec<ContactListRow> = lists
            .into_iter()
            .zip(accounts)
            .map(|(inner, account_id)| ContactListRow { inner, account_id })
            .collect();
        to_json(&rows)
    }

    /// Contacts in a list as a JSON `Contact[]`, routed to the list's owning
    /// account (local store or external provider).
    pub fn contacts_json(&self, list_id: String) -> Result<String, StoreError> {
        let route = self.route_contact_list(&list_id)?;
        let contacts = self
            .runtime
            .block_on(async {
                match route {
                    None => self.adapter.get_contacts(&list_id).await,
                    Some(ext) => ext.get_contacts(&list_id).await,
                }
            })
            .map_err(map_store_err)?;
        to_json(&contacts)
    }

    /// Create a contact from a JSON `cal_core::NewContact`; returns the created
    /// `Contact` as JSON. Routed by `list_id` (local store or external provider).
    pub fn create_contact_json(
        &self,
        list_id: String,
        contact_json: String,
    ) -> Result<String, StoreError> {
        let new: cal_core::NewContact = from_json("contact", &contact_json)?;
        let route = self.route_contact_list(&list_id)?;
        let contact = self
            .runtime
            .block_on(async {
                match route {
                    None => self.adapter.create_contact(&list_id, new).await,
                    Some(ext) => ext.create_contact(&list_id, new).await,
                }
            })
            .map_err(map_store_err)?;
        to_json(&contact)
    }

    /// Update a contact from a JSON `cal_core::Contact`; returns the updated
    /// `Contact` as JSON. Routed by the contact's `list_id`.
    pub fn update_contact_json(&self, contact_json: String) -> Result<String, StoreError> {
        let contact: cal_core::Contact = from_json("contact", &contact_json)?;
        let route = self.route_contact_list(&contact.list_id)?;
        let updated = self
            .runtime
            .block_on(async {
                match route {
                    None => self.adapter.update_contact(contact).await,
                    Some(ext) => ext.update_contact(contact).await,
                }
            })
            .map_err(map_store_err)?;
        to_json(&updated)
    }

    /// Delete a contact, routed by the optional `list_id` (omit → local). Callers
    /// that listed the contact pass its `list_id` so an external delete reaches
    /// the right provider (the desktop `delete_contact` shape).
    pub fn delete_contact(&self, id: String, list_id: Option<String>) -> Result<(), StoreError> {
        let route = match list_id.as_deref() {
            Some(lid) => self.route_contact_list(lid)?,
            None => None,
        };
        self.runtime
            .block_on(async {
                match route {
                    None => self.adapter.delete_contact(&id).await,
                    Some(ext) => ext.delete_contact(&id).await,
                }
            })
            .map_err(map_store_err)
    }

    /// Create a top-level LOCAL address book; returns it as a `ContactListRow`.
    /// Local-only: external contact-list creation isn't a `ContactsFeature`
    /// capability (the desktop `create_contact_list` is local-only too).
    pub fn create_contact_list_json(&self, name: String) -> Result<String, StoreError> {
        let list = self
            .adapter
            .create_contact_list(&name, None, None)
            .map_err(map_store_err)?;
        self.registry.note_contact_list_route(&list.id, LOCAL_ID);
        to_json(&ContactListRow {
            inner: list,
            account_id: LOCAL_ID.to_string(),
        })
    }

    /// Delete a LOCAL address book. Local-only (the inherent store method, which
    /// forbids deleting the seeded default list); matches the desktop.
    pub fn delete_contact_list(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_contact_list(&id).map_err(map_store_err)
    }

    // ── OAuth (host-driven, for mobile native auth sessions) ──────────────────
    //
    // Desktop runs OAuth via the plugin's loopback+browser dance; mobile can't,
    // so the host drives it in two phases around a native auth session. These
    // call the statically-embedded plugin's `interactive_auth` with a `phase`
    // discriminator (the auth-fn is carried by `register_static_with_auth`).
    // `begin` is pure (builds the authorize URL + PKCE — no network); the mobile
    // layer opens that URL, captures the `aperio://` redirect, and calls
    // `complete` (the network token exchange). The host holds the
    // pkce_verifier/state opaquely between the two calls. Wiring the returned
    // tokens into an account row (AccountsRepo + keychain + AccountCreated) is
    // the follow-on `complete_oauth_json` step.

    /// Begin a host-driven OAuth flow for `plugin_id` (e.g.
    /// `com.aperio.cal-adapter-google`). `args_json` carries the provider's
    /// begin inputs — `{client_id, redirect_uri}` (Google) /
    /// `{client_id, authority, redirect_uri}` (Microsoft); the `phase:"authorize"`
    /// discriminator is injected here. Returns the plugin's
    /// `{authorize_url, pkce_verifier, state}` JSON. The caller opens
    /// `authorize_url` in a native auth session and keeps `pkce_verifier` +
    /// `state` for the matching `complete` call.
    pub fn begin_oauth_json(
        &self,
        plugin_id: String,
        args_json: String,
    ) -> Result<String, StoreError> {
        let mut args: serde_json::Value = from_json("oauth begin", &args_json)?;
        let obj = args
            .as_object_mut()
            .ok_or_else(|| StoreError::InvalidField {
                field: "args".to_string(),
                detail: "OAuth begin args must be a JSON object".to_string(),
            })?;
        obj.insert(
            "phase".to_string(),
            serde_json::Value::String("authorize".to_string()),
        );
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .interactive_auth(&plugin_id, &args.to_string())
                    .await
            })
            .map_err(|e| StoreError::Auth {
                detail: e.to_string(),
            })?;
        String::from_utf8(bytes).map_err(|e| StoreError::Protocol {
            detail: format!("authorize response was not UTF-8: {e}"),
        })
    }

    /// Run a plugin's endpoint **discovery** (e.g. EWS Autodiscover for
    /// `com.aperio.cal-adapter-ews`). `args_json` carries the provider's discover
    /// inputs — `{email, password}` for EWS. Returns the plugin's discovered
    /// endpoints as JSON (`{ews_url, account_email}` for EWS) for the caller to
    /// pre-fill the account form. Mirrors the desktop `discover_ews_endpoint`
    /// command; the network call hits the provider's Autodiscover, so a failure
    /// surfaces the plugin's own actionable message ("HTTP 401", "no endpoint for
    /// …") and the UI can fall back to a manually-entered endpoint.
    pub fn discover_json(
        &self,
        plugin_id: String,
        args_json: String,
    ) -> Result<String, StoreError> {
        let args: serde_json::Value = from_json("discover", &args_json)?;
        if !args.is_object() {
            return Err(StoreError::InvalidField {
                field: "args".to_string(),
                detail: "discover args must be a JSON object".to_string(),
            });
        }
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .discover(&plugin_id, &args.to_string())
                    .await
            })
            .map_err(map_discover_err)?;
        String::from_utf8(bytes).map_err(|e| StoreError::Protocol {
            detail: format!("discover response was not UTF-8: {e}"),
        })
    }

    /// Complete a host-driven OAuth flow: exchange the redirect's `code` (+ the
    /// `pkce_verifier`/`state` from [`Self::begin_oauth_json`]) for tokens via
    /// the plugin (`phase:"exchange"`, the network step), then create the
    /// account — persist the row (with the non-secret `config_json`), store the
    /// access + refresh tokens via the keychain bridge (refresh is the durable,
    /// cross-device-syncable credential), register the adapter, and append
    /// `AccountCreated`. Mirrors the desktop `connect_*_account` tail + the
    /// `create_account_json` row/secret/registry/teardown discipline. Returns the
    /// created account as JSON. (The exchange itself is verified on-device — it
    /// hits the provider's token endpoint.)
    pub fn complete_oauth_json(
        &self,
        plugin_id: String,
        request_json: String,
    ) -> Result<String, StoreError> {
        let req: CompleteOAuthRequest = from_json("oauth complete", &request_json)?;

        // 0. Enforce the same non-empty invariants the desktop connect_* commands
        //    check server-side, so the engine boundary holds regardless of caller
        //    (not only when the UI form happens to validate first).
        if req.display_name.trim().is_empty() {
            return Err(StoreError::InvalidField {
                field: "display_name".to_string(),
                detail: "display name must not be empty".to_string(),
            });
        }
        if req.client_id.trim().is_empty() {
            return Err(StoreError::InvalidField {
                field: "client_id".to_string(),
                detail: "client_id must not be empty".to_string(),
            });
        }
        // Google's token endpoint requires the secret; Microsoft (a PKCE public
        // client) carries none, so only gate it for Google.
        if matches!(req.adapter_kind, AdapterKind::Google)
            && req
                .client_secret
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            return Err(StoreError::InvalidField {
                field: "client_secret".to_string(),
                detail: "client_secret must not be empty".to_string(),
            });
        }

        // 1. Exchange the code for tokens FIRST — so a failed exchange never
        //    leaves an orphaned account row. The plugin's exchange phase does
        //    the CSRF (state) check.
        let exchange_args = serde_json::json!({
            "phase": "exchange",
            "client_id": req.client_id,
            // Google needs the secret; Microsoft is a PKCE public client and
            // ignores it. authority selects Microsoft's v2.0 tenant (null for
            // Google, which ignores the field).
            "client_secret": req.client_secret,
            "authority": req.authority,
            "code": req.code,
            "pkce_verifier": req.pkce_verifier,
            "state": req.state,
            "returned_state": req.returned_state,
            "redirect_uri": req.redirect_uri,
        });
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .interactive_auth(&plugin_id, &exchange_args.to_string())
                    .await
            })
            .map_err(|e| StoreError::Auth {
                detail: e.to_string(),
            })?;
        let tokens: OAuthTokenJson =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Protocol {
                detail: format!("token blob: {e}"),
            })?;

        // 2. Persist the account row with the non-secret adapter config.
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let created = repo
            .create(req.adapter_kind, req.display_name.trim(), &req.config_json)
            .map_err(acc_err)?;

        // 3. Store the tokens (device-local keychain). The access token is
        //    ephemeral (re-minted from the refresh token) so it's NOT synced;
        //    the refresh token IS the durable credential, pushed to the user's
        //    other devices via the E2E credential log. Tear the row down on a
        //    keychain failure so DB/keychain/registry never drift.
        if let Err(err) =
            self.secret_store
                .store(&created.id, SecretSlot::AccessToken, &tokens.access_token)
        {
            let _ = repo.delete(&created.id);
            return Err(StoreError::Storage {
                detail: format!("store access token: {err}"),
            });
        }
        if let Some(refresh) = tokens.refresh_token.as_deref() {
            if let Err(err) =
                self.secret_store
                    .store(&created.id, SecretSlot::RefreshToken, refresh)
            {
                let _ = self.secret_store.delete_all(&created.id);
                let _ = repo.delete(&created.id);
                return Err(StoreError::Storage {
                    detail: format!("store refresh token: {err}"),
                });
            }
            host_core::credential_sync::emit_credential_set(
                &self.writer,
                &shared,
                &created.id,
                SecretSlot::RefreshToken,
                refresh,
            );
        }

        // 4. Register the freshly-created adapter. Fatal on failure — drop
        //    secrets + row.
        if let Err(err) = self.registry.register(&created) {
            let _ = self.secret_store.delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(StoreError::Storage {
                detail: format!("adapter registration failed: {err}"),
            });
        }

        // 5. Sync the new account row (non-secret metadata) to other devices.
        self.writer
            .append(SyncEvent::AccountCreated(account_payload(&created)));

        to_json(&created)
    }
}

/// Request body for [`Host::complete_oauth_json`]. The `config_json` is the
/// non-secret adapter config the registry reads back (`{client_id, client_secret}`
/// for Google, `{client_id, authority}` for Microsoft); the remaining fields are
/// the token-exchange inputs forwarded to the plugin's `phase:"exchange"`.
#[derive(serde::Deserialize)]
struct CompleteOAuthRequest {
    adapter_kind: AdapterKind,
    display_name: String,
    config_json: String,
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    /// Microsoft's v2.0 tenant slug (`common` / `organizations` / a GUID).
    /// Absent for Google (which has no authority concept).
    #[serde(default)]
    authority: Option<String>,
    code: String,
    pkce_verifier: String,
    state: String,
    returned_state: String,
    redirect_uri: String,
}

/// The bits of the plugin's token response the host needs to persist.
#[derive(serde::Deserialize)]
struct OAuthTokenJson {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Request body for [`Host::complete_sync_oauth_json`] — the token-exchange
/// inputs forwarded to the sync plugin's `phase:"exchange"`. No account fields
/// (sync OAuth stores only the refresh token in the adapter's keychain slot).
#[derive(serde::Deserialize)]
struct CompleteSyncOAuthRequest {
    client_id: String,
    /// Dropbox: optional (PKCE public app). Google Drive: required.
    #[serde(default)]
    client_secret: Option<String>,
    code: String,
    pkce_verifier: String,
    state: String,
    returned_state: String,
    redirect_uri: String,
}

/// Map an accounts-repo error to the FFI store error, preserving the
/// NotFound / forbidden-local-delete distinctions for the UI.
fn acc_err(e: host_core::accounts::AccountsError) -> StoreError {
    use host_core::accounts::AccountsError;
    match e {
        AccountsError::NotFound(_) => StoreError::NotFound,
        AccountsError::DeleteLocalForbidden => StoreError::InvalidField {
            field: "account_id".to_string(),
            detail: "the local account cannot be deleted".to_string(),
        },
        AccountsError::UnknownKind(detail) => StoreError::InvalidField {
            field: "adapter_kind".to_string(),
            detail,
        },
        AccountsError::Sqlite(e) => StoreError::Storage {
            detail: e.to_string(),
        },
    }
}

/// Map a plugin discover error to the FFI store error. On mobile every plugin is
/// statically embedded, so `PluginMissing` shouldn't occur for a real id;
/// `Unsupported` and the plugin's own failure message (the actionable text, e.g.
/// "Autodiscover HTTP 401" / "no endpoint for …") still need to reach the UI so
/// it can fall back to a manually-entered endpoint.
fn map_discover_err(e: plugin_core::manager::DiscoverError) -> StoreError {
    use plugin_core::manager::DiscoverError as D;
    match e {
        D::PluginMissing(id) => StoreError::Unsupported {
            detail: format!("plugin {id} is not loaded"),
        },
        D::Unsupported(id) => StoreError::Unsupported {
            detail: format!("plugin {id} does not support discovery"),
        },
        D::Plugin(msg) => StoreError::Protocol { detail: msg },
    }
}

/// Map a plugin host-key-probe error to the FFI store error. A failed probe
/// (DNS / connect / SSH handshake) carries the plugin's own message so the UI
/// can show why the fingerprint couldn't be fetched and offer a retry.
fn map_probe_err(e: plugin_core::manager::ProbeHostKeyError) -> StoreError {
    use plugin_core::manager::ProbeHostKeyError as P;
    match e {
        P::PluginMissing(id) => StoreError::Unsupported {
            detail: format!("plugin {id} is not loaded"),
        },
        P::Unsupported(id) => StoreError::Unsupported {
            detail: format!("plugin {id} does not support host-key probing"),
        },
        P::Plugin(msg) => StoreError::Network { detail: msg },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory keychain for tests — the Rust stand-in for the iOS/
    /// Android bridge.
    #[derive(Default)]
    struct FakeKeychain {
        map: Mutex<HashMap<(String, String), String>>,
    }

    impl KeychainBridge for FakeKeychain {
        fn store(
            &self,
            account_id: String,
            slot: String,
            value: String,
        ) -> Result<(), KeychainError> {
            self.map.lock().unwrap().insert((account_id, slot), value);
            Ok(())
        }
        fn retrieve(&self, account_id: String, slot: String) -> Result<String, KeychainError> {
            self.map
                .lock()
                .unwrap()
                .get(&(account_id, slot))
                .cloned()
                .ok_or(KeychainError::NotFound)
        }
        fn delete(&self, account_id: String, slot: String) -> Result<(), KeychainError> {
            self.map.lock().unwrap().remove(&(account_id, slot));
            Ok(())
        }
        fn delete_all(&self, account_id: String) -> Result<(), KeychainError> {
            self.map
                .lock()
                .unwrap()
                .retain(|(acc, _), _| acc != &account_id);
            Ok(())
        }
    }

    fn open_host() -> (tempfile::TempDir, Arc<Host>, Arc<FakeKeychain>) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("aperio.sqlite");
        let keychain = Arc::new(FakeKeychain::default());
        let host = Host::open(
            db_path.to_string_lossy().into_owned(),
            keychain.clone() as Arc<dyn KeychainBridge>,
        )
        .unwrap();
        (dir, host, keychain)
    }

    #[test]
    fn fresh_host_lists_only_the_seeded_local_account() {
        let (_dir, host, _kc) = open_host();
        let json = host.accounts_json().unwrap();
        // Migration 0003 seeds the implicit local account.
        assert!(json.contains("\"adapter_kind\":\"local\""), "got: {json}");
    }

    #[test]
    fn create_caldav_account_persists_row_and_secret_and_registers() {
        let (_dir, host, kc) = open_host();
        let req = r#"{
            "adapter_kind": "caldav",
            "display_name": "Work CalDAV",
            "config_json": "{\"server_url\":\"https://dav.example.invalid/\",\"username\":\"alice\",\"auth_kind\":\"basic\"}",
            "secret": "hunter2"
        }"#;
        let created = host.create_account_json(req.to_string()).unwrap();
        assert!(created.contains("\"adapter_kind\":\"caldav\""));
        assert!(created.contains("\"display_name\":\"Work CalDAV\""));

        // The account is listed.
        let listed = host.accounts_json().unwrap();
        assert!(listed.contains("Work CalDAV"));

        // The secret reached the keychain bridge under the password slot.
        let stored = kc.map.lock().unwrap();
        assert!(stored
            .iter()
            .any(|((_, slot), v)| slot == "password" && v == "hunter2"));
    }

    #[test]
    fn create_then_delete_account_round_trips() {
        let (_dir, host, kc) = open_host();
        let req = r#"{
            "adapter_kind": "vikunja",
            "display_name": "Tasks",
            "config_json": "{\"server_url\":\"https://vikunja.example.invalid/\"}",
            "secret": "tok_123"
        }"#;
        let created = host.create_account_json(req.to_string()).unwrap();
        let id: serde_json::Value = serde_json::from_str(&created).unwrap();
        let account_id = id["id"].as_str().unwrap().to_string();

        // API-token slot for Vikunja.
        assert!(kc
            .map
            .lock()
            .unwrap()
            .keys()
            .any(|(_, slot)| slot == "api_token"));

        host.delete_account(account_id.clone()).unwrap();
        let listed = host.accounts_json().unwrap();
        assert!(!listed.contains("Tasks"));
        // Secrets cleared for the account.
        assert!(kc
            .map
            .lock()
            .unwrap()
            .keys()
            .all(|(acc, _)| acc != &account_id));
    }

    #[test]
    fn oauth_kind_is_rejected_until_a_later_phase() {
        let (_dir, host, _kc) = open_host();
        let req = r#"{"adapter_kind":"google","display_name":"G"}"#;
        let err = host.create_account_json(req.to_string()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidField { .. }));
    }

    #[test]
    fn deleting_the_local_account_is_forbidden() {
        let (_dir, host, _kc) = open_host();
        let err = host.delete_account("local".to_string()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidField { .. }));
    }

    // ─── Calendars ───────────────────────────────────────────────────────────

    fn calendar_id(created_json: &str) -> String {
        serde_json::from_str::<serde_json::Value>(created_json).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn create_calendar_returns_flattened_local_row_and_primes_route() {
        let (_dir, host, _kc) = open_host();
        // No color_label: a label id would need a matching color_labels row
        // (FK), and a fresh DB defines none. The desktop only ever sends an
        // existing label; the null case is what an unlabelled calendar carries.
        let created = host
            .create_calendar_json(r#"{"name":"Trips"}"#.to_string())
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&created).unwrap();
        // CalendarRow flattens Calendar: id/name at the top level, not nested.
        assert!(v["id"].is_string());
        assert_eq!(v["name"], "Trips");
        assert_eq!(v["account_id"], "local");
        // color_label is present-as-null (a bare value, never an object).
        assert!(v["color_label"].is_null());
        // The new calendar routes immediately (no re-list needed).
        let id = v["id"].as_str().unwrap();
        assert_eq!(
            host.registry.account_for_calendar(id),
            Some("local".to_string())
        );
    }

    #[test]
    fn list_calendars_includes_local_and_primes_routes() {
        let (_dir, host, _kc) = open_host();
        let id = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Personal"}"#.to_string())
                .unwrap(),
        );

        let listed = host.list_calendars_json().unwrap();
        let rows: serde_json::Value = serde_json::from_str(&listed).unwrap();
        let arr = rows.as_array().unwrap();
        // The created calendar is present, tagged local.
        assert!(arr
            .iter()
            .any(|r| r["id"] == serde_json::json!(id) && r["account_id"] == "local"));
        // Every row carries the account_id source-grouping key.
        assert!(arr.iter().all(|r| r["account_id"].is_string()));
        // Listing primed the route map.
        assert_eq!(
            host.registry.account_for_calendar(&id),
            Some("local".to_string())
        );
    }

    #[test]
    fn delete_calendar_removes_it() {
        let (_dir, host, _kc) = open_host();
        let id = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Temp"}"#.to_string())
                .unwrap(),
        );
        host.delete_calendar(id).unwrap();
        let listed = host.list_calendars_json().unwrap();
        assert!(!listed.contains("Temp"));
    }

    // ─── Events ──────────────────────────────────────────────────────────────

    fn make_calendar(host: &Host) -> String {
        calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Cal"}"#.to_string())
                .unwrap(),
        )
    }

    /// A minimal valid `CreateEventRequest` JSON (NewEvent flattened). Omits
    /// color_hex + send_invitations (serde defaults); all other Options are
    /// present-as-null, as `cal_core::NewEvent` requires.
    fn new_event_json(cal: &str, title: &str) -> String {
        format!(
            r#"{{"calendar_id":"{cal}","title":"{title}","description":null,"location":null,"start":"2026-06-20T09:00:00Z","end":"2026-06-20T09:30:00Z","all_day":false,"recurrence":null,"color_label":null,"reminders":[],"sound":null,"attendees":[]}}"#
        )
    }

    fn covering_range(cal: &str) -> String {
        format!(
            r#"{{"calendar_id":"{cal}","start":"2026-06-01T00:00:00Z","end":"2026-07-01T00:00:00Z"}}"#
        )
    }

    #[test]
    fn create_event_round_trips_through_get_and_get_by_id() {
        let (_dir, host, _kc) = open_host();
        let cal = make_calendar(&host);
        let created = host
            .create_event_json(new_event_json(&cal, "Standup"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&created).unwrap();
        assert!(v["id"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(v["title"], "Standup");
        assert!(v.get("created_at").is_some());
        // skip_serializing_if keeps these off the wire when unset.
        assert!(v.get("color_hex").is_none());
        assert!(v.get("send_invitations").is_none());
        let id = v["id"].as_str().unwrap().to_string();

        // get_events over a covering range returns it.
        let events: serde_json::Value =
            serde_json::from_str(&host.get_events_json(covering_range(&cal)).unwrap()).unwrap();
        assert!(events
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == serde_json::json!(id)));

        // get_event_by_id returns Some(event).
        let one: serde_json::Value =
            serde_json::from_str(&host.get_event_by_id_json(id.clone()).unwrap()).unwrap();
        assert_eq!(one["id"], serde_json::json!(id));
    }

    #[test]
    fn update_event_changes_title_and_persists() {
        let (_dir, host, _kc) = open_host();
        let cal = make_calendar(&host);
        let created = host.create_event_json(new_event_json(&cal, "Old")).unwrap();
        let mut event: serde_json::Value = serde_json::from_str(&created).unwrap();
        event["title"] = serde_json::json!("New");
        let updated = host.update_event_json(event.to_string()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&updated).unwrap()["title"],
            "New"
        );
        let id = event["id"].as_str().unwrap().to_string();
        let reread: serde_json::Value =
            serde_json::from_str(&host.get_event_by_id_json(id).unwrap()).unwrap();
        assert_eq!(reread["title"], "New");
    }

    #[test]
    fn moving_an_event_between_local_calendars_reroutes_it() {
        let (_dir, host, _kc) = open_host();
        let cal_a = make_calendar(&host);
        let cal_b = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"B"}"#.to_string())
                .unwrap(),
        );
        let created = host
            .create_event_json(new_event_json(&cal_a, "Movable"))
            .unwrap();
        let mut event: serde_json::Value = serde_json::from_str(&created).unwrap();
        let id = event["id"].as_str().unwrap().to_string();

        // A local→local move is an in-place update routed by event.calendar_id
        // (the desktop treats it as a single SQL UPDATE).
        event["calendar_id"] = serde_json::json!(cal_b);
        host.update_event_json(event.to_string()).unwrap();

        // B now contains the event; A no longer does.
        let in_b: serde_json::Value =
            serde_json::from_str(&host.get_events_json(covering_range(&cal_b)).unwrap()).unwrap();
        assert!(in_b
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == serde_json::json!(id)));
        let in_a: serde_json::Value =
            serde_json::from_str(&host.get_events_json(covering_range(&cal_a)).unwrap()).unwrap();
        assert!(in_a
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["id"] != serde_json::json!(id)));
    }

    #[test]
    fn delete_event_removes_it() {
        let (_dir, host, _kc) = open_host();
        let cal = make_calendar(&host);
        let created = host
            .create_event_json(new_event_json(&cal, "Doomed"))
            .unwrap();
        let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        host.delete_event(id.clone(), Some(cal), None).unwrap();
        // get_event_by_id → JSON null after deletion.
        assert_eq!(host.get_event_by_id_json(id).unwrap().trim(), "null");
    }

    #[test]
    fn updating_a_deleted_event_is_not_found() {
        let (_dir, host, _kc) = open_host();
        let cal = make_calendar(&host);
        let created = host
            .create_event_json(new_event_json(&cal, "Ghost"))
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(&created).unwrap();
        let id = event["id"].as_str().unwrap().to_string();
        host.delete_event(id, Some(cal), None).unwrap();
        let err = host.update_event_json(event.to_string()).unwrap_err();
        assert!(matches!(err, StoreError::NotFound));
    }

    #[test]
    fn get_events_for_unknown_calendar_routes_local_and_returns_empty() {
        let (_dir, host, _kc) = open_host();
        let range = r#"{"calendar_id":"does-not-exist","start":"2026-06-01T00:00:00Z","end":"2026-07-01T00:00:00Z"}"#;
        let events: serde_json::Value =
            serde_json::from_str(&host.get_events_json(range.to_string()).unwrap()).unwrap();
        assert!(events.as_array().unwrap().is_empty());
    }

    // ─── Sync (writer + status) ──────────────────────────────────────────────

    #[test]
    fn local_mutations_populate_the_pending_log_and_status_is_unconfigured() {
        let (dir, host, _kc) = open_host();
        // No sync adapter configured yet.
        let status: serde_json::Value =
            serde_json::from_str(&host.sync_status_json().unwrap()).unwrap();
        assert_eq!(status["configured"], false);

        // Two local mutations → CalendarCreated + EventCreated in the log.
        let cal = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Sync me"}"#.to_string())
                .unwrap(),
        );
        host.create_event_json(new_event_json(&cal, "Logged"))
            .unwrap();

        // The writer drains on the runtime worker thread; poll briefly for the
        // pending session file to materialise (bounded so a stall fails loudly).
        let pending = dir.path().join("sync").join("log").join("pending");
        let mut bytes = 0u64;
        for _ in 0..40 {
            if let Ok(entries) = std::fs::read_dir(&pending) {
                bytes = entries
                    .flatten()
                    .filter_map(|e| std::fs::metadata(e.path()).ok())
                    .map(|m| m.len())
                    .sum();
            }
            if bytes > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            bytes > 0,
            "expected a non-empty pending log file after local mutations",
        );
    }

    /// Open a Host at `<dir>/<name>.sqlite` with a fresh fake keychain.
    fn open_named(dir: &tempfile::TempDir, name: &str) -> Arc<Host> {
        Host::open(
            dir.path()
                .join(format!("{name}.sqlite"))
                .to_string_lossy()
                .into_owned(),
            Arc::new(FakeKeychain::default()) as Arc<dyn KeychainBridge>,
        )
        .unwrap()
    }

    /// Poll (bounded) until the Host's pending sync dir has a non-empty log, so
    /// the writer's async drain has flushed before we push.
    fn wait_for_pending(dir: &tempfile::TempDir) {
        let pending = dir.path().join("sync").join("log").join("pending");
        for _ in 0..40 {
            let bytes: u64 = std::fs::read_dir(&pending)
                .map(|es| {
                    es.flatten()
                        .filter_map(|e| std::fs::metadata(e.path()).ok())
                        .map(|m| m.len())
                        .sum()
                })
                .unwrap_or(0);
            if bytes > 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn two_hosts_sync_an_event_through_a_local_target() {
        // The shared local-filesystem sync target both Hosts point at.
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );

        let dir_a = tempfile::tempdir().unwrap();
        let host_a = open_named(&dir_a, "a");
        let dir_b = tempfile::tempdir().unwrap();
        let host_b = open_named(&dir_b, "b");

        host_a.configure_sync_adapter_json(cfg.clone()).unwrap();
        host_b.configure_sync_adapter_json(cfg).unwrap();
        // Both are configured now.
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&host_a.sync_status_json().unwrap()).unwrap()
                ["configured"],
            true
        );

        // A creates a calendar + event (CalendarCreated + EventCreated logged).
        let cal = calendar_id(
            &host_a
                .create_calendar_json(r#"{"name":"Shared"}"#.to_string())
                .unwrap(),
        );
        let created = host_a
            .create_event_json(new_event_json(&cal, "Across devices"))
            .unwrap();
        let event_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // A pushes its pending logs to the shared target; B fetches + applies.
        wait_for_pending(&dir_a);
        host_a.sync_now_json().unwrap();
        host_b.sync_now_json().unwrap();

        // B now has the calendar (from CalendarCreated) + the event (from
        // EventCreated) — the core mobile↔mobile parity assertion. B'd never
        // listed calendars, so the event routes local (unknown id → local).
        let events: serde_json::Value =
            serde_json::from_str(&host_b.get_events_json(covering_range(&cal)).unwrap()).unwrap();
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == serde_json::json!(event_id)),
            "Host B should see A's event after a sync round; got: {events}",
        );

        // Idempotency: a second round on B applies nothing new (no panic, no dup).
        host_b.sync_now_json().unwrap();
        let again: serde_json::Value =
            serde_json::from_str(&host_b.get_events_json(covering_range(&cal)).unwrap()).unwrap();
        assert_eq!(
            again
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["id"] == serde_json::json!(event_id))
                .count(),
            1,
            "the event must appear exactly once after a repeat round",
        );
    }

    #[test]
    fn two_hosts_sync_a_task_through_a_local_target() {
        // The same mobile↔mobile parity assertion as the event capstone, but for
        // the task surface — proves task/list mutations append to the sync log
        // and round-trip through a shared local-filesystem target.
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );

        let dir_a = tempfile::tempdir().unwrap();
        let host_a = open_named(&dir_a, "a");
        let dir_b = tempfile::tempdir().unwrap();
        let host_b = open_named(&dir_b, "b");

        host_a.configure_sync_adapter_json(cfg.clone()).unwrap();
        host_b.configure_sync_adapter_json(cfg).unwrap();

        // A creates a list (TaskListCreated) + a task in it (TaskCreated).
        let list = host_a.create_task_list_json("Shared".to_string()).unwrap();
        let list_id = serde_json::from_str::<serde_json::Value>(&list).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let new_task = r#"{"title":"Across devices","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let created = host_a
            .create_task_json(list_id.clone(), new_task.to_string())
            .unwrap();
        let task_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // A pushes its pending logs; B fetches + applies (list before task, in
        // append order, so the task's FK to its list is satisfied).
        wait_for_pending(&dir_a);
        host_a.sync_now_json().unwrap();
        host_b.sync_now_json().unwrap();

        let lists: serde_json::Value =
            serde_json::from_str(&host_b.task_lists_json().unwrap()).unwrap();
        assert!(
            lists
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["id"] == serde_json::json!(list_id)),
            "Host B should see A's task list after a sync round; got: {lists}",
        );
        let tasks: serde_json::Value =
            serde_json::from_str(&host_b.tasks_json(list_id.clone()).unwrap()).unwrap();
        assert!(
            tasks
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["id"] == serde_json::json!(task_id)),
            "Host B should see A's task after a sync round; got: {tasks}",
        );

        // Idempotency: a repeat round adds no duplicate.
        host_b.sync_now_json().unwrap();
        let again: serde_json::Value =
            serde_json::from_str(&host_b.tasks_json(list_id).unwrap()).unwrap();
        assert_eq!(
            again
                .as_array()
                .unwrap()
                .iter()
                .filter(|t| t["id"] == serde_json::json!(task_id))
                .count(),
            1,
            "the task must appear exactly once after a repeat round",
        );
    }

    #[test]
    fn configured_local_target_is_restored_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir
            .path()
            .join("aperio.sqlite")
            .to_string_lossy()
            .into_owned();
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );

        // Configure on a first Host, then drop it.
        {
            let host = Host::open(
                db_path.clone(),
                Arc::new(FakeKeychain::default()) as Arc<dyn KeychainBridge>,
            )
            .unwrap();
            host.configure_sync_adapter_json(cfg).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&host.sync_status_json().unwrap())
                    .unwrap()["configured"],
                true
            );
        }

        // Reopening at the same db path restores the target from prefs.
        let host2 = Host::open(
            db_path,
            Arc::new(FakeKeychain::default()) as Arc<dyn KeychainBridge>,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&host2.sync_status_json().unwrap()).unwrap()
                ["configured"],
            true,
            "the local sync target should be restored on reopen",
        );
    }

    #[test]
    fn enabling_e2e_encrypts_pushed_logs_and_restores_on_reopen() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir
            .path()
            .join("aperio.sqlite")
            .to_string_lossy()
            .into_owned();
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        // One shared in-memory keychain so the E2E key survives the reopen below
        // (a fresh FakeKeychain would lose it — exercising a different path).
        let kc = Arc::new(FakeKeychain::default());
        const TITLE: &str = "TopSecretRendezvous";

        {
            let host = Host::open(db_path.clone(), kc.clone() as Arc<dyn KeychainBridge>).unwrap();
            host.configure_sync_adapter_json(cfg.clone()).unwrap();
            let cal = calendar_id(
                &host
                    .create_calendar_json(r#"{"name":"Secret"}"#.to_string())
                    .unwrap(),
            );
            host.create_event_json(new_event_json(&cal, TITLE)).unwrap();
            // Turn on E2E, then push the pending logs (now encrypted).
            host.enable_sync_encryption_json("correct horse battery".to_string())
                .unwrap();
            let status: serde_json::Value =
                serde_json::from_str(&host.sync_status_json().unwrap()).unwrap();
            assert_eq!(status["e2e_enabled"], serde_json::json!(true));
            wait_for_pending(&dir);
            host.sync_now_json().unwrap();
        }

        // Nothing the sync pushed may carry the event title in plaintext — every
        // log + snapshot body is AES-GCM ciphertext (only meta.json stays plain,
        // and it holds no event data).
        fn collect(dir: &std::path::Path, out: &mut Vec<u8>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    collect(&p, &mut *out);
                } else if let Ok(bytes) = fs::read(&p) {
                    out.extend_from_slice(&bytes);
                }
            }
        }
        let mut all = Vec::new();
        collect(remote.path(), &mut all);
        assert!(!all.is_empty(), "the sync push should have written files");
        assert!(
            !all.windows(TITLE.len()).any(|w| w == TITLE.as_bytes()),
            "the event title must never appear in plaintext on an E2E target",
        );

        // Reopen with the SAME db + keychain: restore re-wraps with the stored
        // key, and a fetch round decrypts its own logs without error.
        let host2 = Host::open(db_path, kc as Arc<dyn KeychainBridge>).unwrap();
        let status2: serde_json::Value =
            serde_json::from_str(&host2.sync_status_json().unwrap()).unwrap();
        assert_eq!(
            status2["configured"],
            serde_json::json!(true),
            "the E2E target should be restored on reopen"
        );
        assert_eq!(status2["e2e_enabled"], serde_json::json!(true));
        // Decrypts the previously-pushed (own) logs without error.
        host2.sync_now_json().unwrap();
    }

    #[test]
    fn reconfiguring_an_e2e_target_keeps_it_encrypted() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir
            .path()
            .join("aperio.sqlite")
            .to_string_lossy()
            .into_owned();
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        let kc = Arc::new(FakeKeychain::default());
        const TITLE1: &str = "FirstSecretMeeting";
        const TITLE2: &str = "SecondSecretMeeting";

        let host = Host::open(db_path, kc as Arc<dyn KeychainBridge>).unwrap();
        host.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Secret"}"#.to_string())
                .unwrap(),
        );
        host.create_event_json(new_event_json(&cal, TITLE1))
            .unwrap();
        host.enable_sync_encryption_json("correct horse battery".to_string())
            .unwrap();
        wait_for_pending(&dir);
        host.sync_now_json().unwrap();

        // Re-point at the SAME (now-encrypted) target — the regression guard:
        // wrap_for_target must read the target meta, see it's E2E, and re-wrap
        // with the device-local key. Before the fix this configured a PLAINTEXT
        // adapter, leaking every subsequent log into the encrypted dataset.
        host.configure_sync_adapter_json(cfg).unwrap();
        let status: serde_json::Value =
            serde_json::from_str(&host.sync_status_json().unwrap()).unwrap();
        assert_eq!(
            status["e2e_enabled"],
            serde_json::json!(true),
            "reconfiguring an encrypted target must keep it encrypted"
        );

        // A fresh event pushed AFTER the reconfigure must also be ciphertext.
        host.create_event_json(new_event_json(&cal, TITLE2))
            .unwrap();
        wait_for_pending(&dir);
        host.sync_now_json().unwrap();

        fn collect(dir: &std::path::Path, out: &mut Vec<u8>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    collect(&p, &mut *out);
                } else if let Ok(bytes) = fs::read(&p) {
                    out.extend_from_slice(&bytes);
                }
            }
        }
        let mut all = Vec::new();
        collect(remote.path(), &mut all);
        for title in [TITLE1, TITLE2] {
            assert!(
                !all.windows(title.len()).any(|w| w == title.as_bytes()),
                "{title} leaked in plaintext on the E2E target after a reconfigure",
            );
        }
    }

    #[test]
    fn enable_e2e_reencrypts_an_already_populated_plaintext_target() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir
            .path()
            .join("aperio.sqlite")
            .to_string_lossy()
            .into_owned();
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        let kc = Arc::new(FakeKeychain::default());
        const TITLE: &str = "PopulatedPlaintextSecret";

        let host = Host::open(db_path, kc as Arc<dyn KeychainBridge>).unwrap();
        host.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Diary"}"#.to_string())
                .unwrap(),
        );
        host.create_event_json(new_event_json(&cal, TITLE)).unwrap();
        // Sync FIRST → the event lands on the remote as PLAINTEXT.
        wait_for_pending(&dir);
        host.sync_now_json().unwrap();

        fn collect(dir: &std::path::Path, out: &mut Vec<u8>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    collect(&p, &mut *out);
                } else if let Ok(bytes) = fs::read(&p) {
                    out.extend_from_slice(&bytes);
                }
            }
        }
        let mut before = Vec::new();
        collect(remote.path(), &mut before);
        assert!(
            before.windows(TITLE.len()).any(|w| w == TITLE.as_bytes()),
            "precondition: the title is plaintext on the remote before enabling",
        );

        // Enable on the now-POPULATED plaintext target → re-encrypt in place.
        let report: serde_json::Value = serde_json::from_str(
            &host
                .enable_sync_encryption_json("correct horse battery".to_string())
                .unwrap(),
        )
        .unwrap();
        assert!(
            report["logs_rewritten"].as_u64().unwrap() >= 1,
            "at least the event log must be rewritten as ciphertext; got: {report}",
        );
        let status: serde_json::Value =
            serde_json::from_str(&host.sync_status_json().unwrap()).unwrap();
        assert_eq!(status["e2e_enabled"], serde_json::json!(true));

        // The previously-plaintext title must now be ciphertext everywhere.
        let mut after = Vec::new();
        collect(remote.path(), &mut after);
        assert!(
            !after.windows(TITLE.len()).any(|w| w == TITLE.as_bytes()),
            "the title must no longer appear in plaintext after re-encrypting",
        );
    }

    #[test]
    fn disable_e2e_rewrites_the_dataset_back_to_plaintext() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir
            .path()
            .join("aperio.sqlite")
            .to_string_lossy()
            .into_owned();
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        let kc = Arc::new(FakeKeychain::default());
        const TITLE: &str = "DowngradeMeBackToPlaintext";

        let host = Host::open(db_path, kc as Arc<dyn KeychainBridge>).unwrap();
        host.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Diary"}"#.to_string())
                .unwrap(),
        );
        host.create_event_json(new_event_json(&cal, TITLE)).unwrap();
        // Enable E2E (fresh adopt) + push → ciphertext on the remote.
        host.enable_sync_encryption_json("correct horse battery".to_string())
            .unwrap();
        wait_for_pending(&dir);
        host.sync_now_json().unwrap();

        fn collect(dir: &std::path::Path, out: &mut Vec<u8>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    collect(&p, &mut *out);
                } else if let Ok(bytes) = fs::read(&p) {
                    out.extend_from_slice(&bytes);
                }
            }
        }
        let mut encrypted = Vec::new();
        collect(remote.path(), &mut encrypted);
        assert!(
            !encrypted
                .windows(TITLE.len())
                .any(|w| w == TITLE.as_bytes()),
            "precondition: the title is ciphertext on the remote while E2E is on",
        );

        // Disable with the passphrase → rewrite the dataset as plaintext.
        let report: serde_json::Value = serde_json::from_str(
            &host
                .disable_sync_encryption_json("correct horse battery".to_string())
                .unwrap(),
        )
        .unwrap();
        assert!(
            report["logs_rewritten"].as_u64().unwrap() >= 1,
            "got: {report}"
        );
        let status: serde_json::Value =
            serde_json::from_str(&host.sync_status_json().unwrap()).unwrap();
        assert_eq!(status["e2e_enabled"], serde_json::json!(false));

        // The title is readable plaintext on the remote again.
        let mut plain = Vec::new();
        collect(remote.path(), &mut plain);
        assert!(
            plain.windows(TITLE.len()).any(|w| w == TITLE.as_bytes()),
            "the title must be plaintext on the remote after disabling",
        );
        // A follow-up round still works on the now-plaintext dataset.
        host.sync_now_json().unwrap();
    }

    #[test]
    fn disable_e2e_rejects_empty_and_unencrypted() {
        let (_dir, host, _kc) = open_host();
        // Empty passphrase rejected before touching the adapter.
        assert!(matches!(
            host.disable_sync_encryption_json("   ".to_string()).unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "passphrase"
        ));
        // No sync target configured → InvalidField{field:"sync"}.
        assert!(matches!(
            host.disable_sync_encryption_json("pp".to_string()).unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "sync"
        ));
    }

    #[test]
    fn preview_reports_empty_then_existing_for_an_e2e_dataset() {
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        let dir = tempfile::tempdir().unwrap();
        let host = Host::open(
            dir.path().join("a.sqlite").to_string_lossy().into_owned(),
            Arc::new(FakeKeychain::default()) as Arc<dyn KeychainBridge>,
        )
        .unwrap();

        // A pristine target previews as Empty (no meta.json yet).
        let pv: serde_json::Value =
            serde_json::from_str(&host.preview_sync_target_json(cfg.clone()).unwrap()).unwrap();
        assert_eq!(pv["kind"], serde_json::json!("empty"));

        // Adopt a fresh E2E dataset, then push so meta.json lands.
        host.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host
                .create_calendar_json(r#"{"name":"Secret"}"#.to_string())
                .unwrap(),
        );
        host.create_event_json(new_event_json(&cal, "JoinSecret"))
            .unwrap();
        host.enable_sync_encryption_json("correct horse battery".to_string())
            .unwrap();
        wait_for_pending(&dir);
        host.sync_now_json().unwrap();

        // Now it previews as Existing + encrypted (side-effect-free probe).
        let pv2: serde_json::Value =
            serde_json::from_str(&host.preview_sync_target_json(cfg).unwrap()).unwrap();
        assert_eq!(pv2["kind"], serde_json::json!("existing"));
        assert_eq!(pv2["e2e_enabled"], serde_json::json!(true));
    }

    #[test]
    fn second_device_joins_an_encrypted_dataset_via_passphrase() {
        const PASSPHRASE: &str = "correct horse battery";
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );

        // Device A adopts a fresh E2E dataset, creates a calendar + event, pushes.
        let dir_a = tempfile::tempdir().unwrap();
        let host_a = open_named(&dir_a, "a");
        host_a.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host_a
                .create_calendar_json(r#"{"name":"Shared"}"#.to_string())
                .unwrap(),
        );
        let created = host_a
            .create_event_json(new_event_json(&cal, "Across encrypted devices"))
            .unwrap();
        let event_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        host_a
            .enable_sync_encryption_json(PASSPHRASE.to_string())
            .unwrap();
        wait_for_pending(&dir_a);
        host_a.sync_now_json().unwrap();

        // A wrong passphrase is rejected before any data is applied.
        let dir_wrong = tempfile::tempdir().unwrap();
        let host_wrong = open_named(&dir_wrong, "wrong");
        assert!(
            host_wrong
                .accept_remote_dataset_json(
                    cfg.clone(),
                    Some("Wrong".to_string()),
                    Some("not the passphrase".to_string()),
                )
                .is_err(),
            "joining with the wrong passphrase must fail",
        );

        // Device B joins with the correct passphrase: it derives the key from
        // meta.json, applies the (decrypted) snapshot + logs, and sees A's event.
        let dir_b = tempfile::tempdir().unwrap();
        let host_b = open_named(&dir_b, "b");
        host_b
            .accept_remote_dataset_json(
                cfg,
                Some("Phone B".to_string()),
                Some(PASSPHRASE.to_string()),
            )
            .unwrap();
        let status: serde_json::Value =
            serde_json::from_str(&host_b.sync_status_json().unwrap()).unwrap();
        assert_eq!(
            status["e2e_enabled"],
            serde_json::json!(true),
            "the joined device must be in E2E mode"
        );
        let events: serde_json::Value =
            serde_json::from_str(&host_b.get_events_json(covering_range(&cal)).unwrap()).unwrap();
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == serde_json::json!(event_id)),
            "Host B should see A's event after joining the encrypted dataset; got: {events}",
        );
        // A follow-up round decrypts its own + A's logs without error.
        host_b.sync_now_json().unwrap();
    }

    #[test]
    fn changing_the_passphrase_lets_a_new_device_join_with_the_new_one_only() {
        const OLD: &str = "old correct horse";
        const NEW: &str = "new battery staple";
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );

        // Device A adopts a fresh E2E dataset with the OLD passphrase + pushes.
        let dir_a = tempfile::tempdir().unwrap();
        let host_a = open_named(&dir_a, "a");
        host_a.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host_a
                .create_calendar_json(r#"{"name":"Shared"}"#.to_string())
                .unwrap(),
        );
        let created = host_a
            .create_event_json(new_event_json(&cal, "Rotate me"))
            .unwrap();
        let event_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        host_a.enable_sync_encryption_json(OLD.to_string()).unwrap();
        wait_for_pending(&dir_a);
        host_a.sync_now_json().unwrap();

        // Rotate the passphrase; A keeps working (its keychain DEK is unchanged).
        host_a
            .change_sync_passphrase_json(OLD.to_string(), NEW.to_string())
            .unwrap();
        host_a.sync_now_json().unwrap();

        // The OLD passphrase no longer unwraps the (re-wrapped) key.
        let dir_old = tempfile::tempdir().unwrap();
        let host_old = open_named(&dir_old, "old");
        assert!(
            host_old
                .accept_remote_dataset_json(cfg.clone(), None, Some(OLD.to_string()))
                .is_err(),
            "joining with the rotated-away passphrase must fail",
        );

        // The NEW passphrase joins and decrypts A's event.
        let dir_b = tempfile::tempdir().unwrap();
        let host_b = open_named(&dir_b, "b");
        host_b
            .accept_remote_dataset_json(cfg, Some("Phone B".to_string()), Some(NEW.to_string()))
            .unwrap();
        let events: serde_json::Value =
            serde_json::from_str(&host_b.get_events_json(covering_range(&cal)).unwrap()).unwrap();
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == serde_json::json!(event_id)),
            "the new device must decrypt A's event after the passphrase change; got: {events}",
        );
    }

    #[test]
    fn change_passphrase_rejects_empty_and_unconfigured() {
        let (_dir, host, _kc) = open_host();
        // Empty new passphrase is rejected before touching the adapter.
        assert!(matches!(
            host.change_sync_passphrase_json("old".to_string(), "  ".to_string())
                .unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "new_passphrase"
        ));
        // No sync target configured → InvalidField{field:"sync"}.
        assert!(matches!(
            host.change_sync_passphrase_json("old".to_string(), "new".to_string())
                .unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "sync"
        ));
    }

    #[test]
    fn a_plaintext_device_adopts_encryption_a_peer_turned_on() {
        const PASS: &str = "correct horse battery";
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );

        // Device A configures the target and creates a calendar + event.
        let dir_a = tempfile::tempdir().unwrap();
        let host_a = open_named(&dir_a, "a");
        host_a.configure_sync_adapter_json(cfg.clone()).unwrap();
        let cal = calendar_id(
            &host_a
                .create_calendar_json(r#"{"name":"Shared"}"#.to_string())
                .unwrap(),
        );
        let created = host_a
            .create_event_json(new_event_json(&cal, "Adopt me"))
            .unwrap();
        let event_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Device B configures the SAME target in PLAINTEXT (no round yet).
        let dir_b = tempfile::tempdir().unwrap();
        let host_b = open_named(&dir_b, "b");
        host_b.configure_sync_adapter_json(cfg.clone()).unwrap();

        // A turns on E2E + pushes the (now-encrypted) dataset.
        host_a
            .enable_sync_encryption_json(PASS.to_string())
            .unwrap();
        wait_for_pending(&dir_a);
        host_a.sync_now_json().unwrap();

        // B's next round hits the encryption gate: the target is encrypted but B
        // is still plaintext → EncryptionRequired, latched as the error code.
        assert!(
            host_b.sync_now_json().is_err(),
            "a plaintext device must refuse a round against an encrypted target",
        );
        let status: serde_json::Value =
            serde_json::from_str(&host_b.sync_status_json().unwrap()).unwrap();
        assert_eq!(
            status["last_error_code"],
            serde_json::json!("encryption_required"),
            "the gate must latch encryption_required for the adopt banner; got: {status}",
        );

        // B adopts encryption with the passphrase, then a round succeeds and B
        // sees A's event decrypted.
        host_b
            .adopt_remote_encryption_json(PASS.to_string())
            .unwrap();
        host_b.sync_now_json().unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&host_b.sync_status_json().unwrap()).unwrap();
        assert_eq!(after["e2e_enabled"], serde_json::json!(true));
        assert_eq!(
            after["last_error_code"],
            serde_json::json!(null),
            "a successful round must clear the latch"
        );
        let events: serde_json::Value =
            serde_json::from_str(&host_b.get_events_json(covering_range(&cal)).unwrap()).unwrap();
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == serde_json::json!(event_id)),
            "B must decrypt A's event after adopting encryption; got: {events}",
        );
    }

    #[test]
    fn adopt_remote_encryption_rejects_empty_and_unconfigured() {
        let (_dir, host, _kc) = open_host();
        assert!(matches!(
            host.adopt_remote_encryption_json("  ".to_string()).unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "passphrase"
        ));
        assert!(matches!(
            host.adopt_remote_encryption_json("pp".to_string()).unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "sync"
        ));
    }

    #[test]
    fn color_labels_crud_round_trips() {
        let (_dir, host, _kc) = open_host();
        // Create a named label.
        let created: serde_json::Value = serde_json::from_str(
            &host
                .create_color_label_json("Work".to_string(), "#e53935".to_string())
                .unwrap(),
        )
        .unwrap();
        let id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["name"], "Work");
        assert_eq!(created["hex"], "#e53935");
        assert_eq!(created["ad_hoc"], serde_json::json!(false));
        // It shows in the list.
        let list: serde_json::Value =
            serde_json::from_str(&host.list_color_labels_json().unwrap()).unwrap();
        assert!(list
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["id"] == serde_json::json!(id)),);
        // Rename + recolour.
        let payload =
            serde_json::json!({"id": id, "name": "Job", "hex": "#43a047", "ad_hoc": false})
                .to_string();
        let after: serde_json::Value =
            serde_json::from_str(&host.update_color_label_json(payload).unwrap()).unwrap();
        assert_eq!(after["name"], "Job");
        assert_eq!(after["hex"], "#43a047");
        // Ad-hoc dedups by hex.
        let ad1: serde_json::Value = serde_json::from_str(
            &host
                .get_or_create_ad_hoc_color_label_json("#123456".to_string())
                .unwrap(),
        )
        .unwrap();
        let ad2: serde_json::Value = serde_json::from_str(
            &host
                .get_or_create_ad_hoc_color_label_json("#123456".to_string())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            ad1["id"], ad2["id"],
            "same hex must dedup to one ad-hoc label"
        );
        assert_eq!(ad1["ad_hoc"], serde_json::json!(true));
        // Delete the named one.
        host.delete_color_label(id.clone()).unwrap();
        let list2: serde_json::Value =
            serde_json::from_str(&host.list_color_labels_json().unwrap()).unwrap();
        assert!(
            !list2
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["id"] == serde_json::json!(id)),
            "the deleted label must be gone",
        );
    }

    #[test]
    fn set_container_color_label_binds_local_list_and_calendar() {
        let (_dir, host, _kc) = open_host();
        // A named label to bind.
        let label: serde_json::Value = serde_json::from_str(
            &host
                .create_color_label_json("Work".to_string(), "#e53935".to_string())
                .unwrap(),
        )
        .unwrap();
        let label_id = label["id"].as_str().unwrap().to_string();

        // ── Local task list ──
        let list: serde_json::Value =
            serde_json::from_str(&host.create_task_list_json("Inbox".to_string()).unwrap())
                .unwrap();
        let list_id = list["id"].as_str().unwrap().to_string();
        // Prime the route map so is_local_task_list resolves.
        let _ = host.task_lists_json().unwrap();

        host.set_container_color_label(
            list_id.clone(),
            "task_list".to_string(),
            Some(label_id.clone()),
        )
        .unwrap();
        let lists: serde_json::Value =
            serde_json::from_str(&host.task_lists_json().unwrap()).unwrap();
        let bound = lists
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == serde_json::json!(list_id))
            .unwrap();
        assert_eq!(bound["color_label"], serde_json::json!(label_id));

        // Clearing it (None) drops the binding.
        host.set_container_color_label(list_id.clone(), "task_list".to_string(), None)
            .unwrap();
        let lists: serde_json::Value =
            serde_json::from_str(&host.task_lists_json().unwrap()).unwrap();
        let cleared = lists
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == serde_json::json!(list_id))
            .unwrap();
        assert_eq!(cleared["color_label"], serde_json::Value::Null);

        // ── Local calendar ──
        let cal: serde_json::Value = serde_json::from_str(
            &host
                .create_calendar_json(serde_json::json!({"name": "Personal"}).to_string())
                .unwrap(),
        )
        .unwrap();
        let cal_id = cal["id"].as_str().unwrap().to_string();
        host.set_container_color_label(
            cal_id.clone(),
            "calendar".to_string(),
            Some(label_id.clone()),
        )
        .unwrap();
        let cals: serde_json::Value =
            serde_json::from_str(&host.list_calendars_json().unwrap()).unwrap();
        let bound_cal = cals
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == serde_json::json!(cal_id))
            .unwrap();
        assert_eq!(bound_cal["color_label"], serde_json::json!(label_id));

        // A contact list (even local) now binds its colour via a host-local
        // override (covered in depth by contact_list_colour_and_rename_*).
        host.set_container_color_label(
            "whatever".to_string(),
            "contact_list".to_string(),
            Some(label_id),
        )
        .unwrap();
    }

    #[test]
    fn rename_container_renames_local_list_and_calendar() {
        let (_dir, host, _kc) = open_host();

        // Local task list.
        let list: serde_json::Value =
            serde_json::from_str(&host.create_task_list_json("Inbox".to_string()).unwrap())
                .unwrap();
        let list_id = list["id"].as_str().unwrap().to_string();
        let _ = host.task_lists_json().unwrap(); // prime the route map
        host.rename_container(
            list_id.clone(),
            "task_list".to_string(),
            "  Errands  ".to_string(),
        )
        .unwrap();
        let lists: serde_json::Value =
            serde_json::from_str(&host.task_lists_json().unwrap()).unwrap();
        let renamed = lists
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == serde_json::json!(list_id))
            .unwrap();
        assert_eq!(renamed["name"], "Errands", "name is trimmed + persisted");

        // Local calendar.
        let cal: serde_json::Value = serde_json::from_str(
            &host
                .create_calendar_json(serde_json::json!({"name": "Personal"}).to_string())
                .unwrap(),
        )
        .unwrap();
        let cal_id = cal["id"].as_str().unwrap().to_string();
        host.rename_container(cal_id.clone(), "calendar".to_string(), "Work".to_string())
            .unwrap();
        let cals: serde_json::Value =
            serde_json::from_str(&host.list_calendars_json().unwrap()).unwrap();
        let renamed_cal = cals
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == serde_json::json!(cal_id))
            .unwrap();
        assert_eq!(renamed_cal["name"], "Work");

        // Empty name → InvalidField.
        assert!(matches!(
            host.rename_container(list_id, "task_list".to_string(), "   ".to_string())
                .unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "name"
        ));

        // Contact lists rename via the OverridesRepo path → Unsupported.
        assert!(matches!(
            host.rename_container(
                "whatever".to_string(),
                "contact_list".to_string(),
                "X".to_string()
            )
            .unwrap_err(),
            StoreError::Unsupported { .. }
        ));
    }

    #[test]
    fn contact_list_colour_and_rename_use_host_local_overrides() {
        let (_dir, host, _kc) = open_host();
        let label: serde_json::Value = serde_json::from_str(
            &host
                .create_color_label_json("Work".to_string(), "#e53935".to_string())
                .unwrap(),
        )
        .unwrap();
        let label_id = label["id"].as_str().unwrap().to_string();

        // A contact list always binds its colour via the override (never a row),
        // even though it's local.
        host.set_container_color_label(
            "contacts:work".to_string(),
            "contact_list".to_string(),
            Some(label_id.clone()),
        )
        .unwrap();
        {
            let shared = host.db.shared();
            let repo = OverridesRepo::new(&shared);
            assert!(repo
                .list_color_overrides()
                .unwrap()
                .iter()
                .any(|o| o.container_id == "contacts:work" && o.color_label_id == label_id));
        }
        // Clearing removes the override row.
        host.set_container_color_label(
            "contacts:work".to_string(),
            "contact_list".to_string(),
            None,
        )
        .unwrap();
        {
            let shared = host.db.shared();
            let repo = OverridesRepo::new(&shared);
            assert!(repo
                .list_color_overrides()
                .unwrap()
                .iter()
                .all(|o| o.container_id != "contacts:work"));
        }

        // Renaming a contact list is unsupported (no provider path + the
        // name-override table excludes contact lists).
        assert!(matches!(
            host.rename_container(
                "contacts:work".to_string(),
                "contact_list".to_string(),
                "Team".to_string()
            )
            .unwrap_err(),
            StoreError::Unsupported { .. }
        ));

        // An unknown kind is rejected.
        assert!(matches!(
            host.set_container_color_label("x".to_string(), "bogus".to_string(), None)
                .unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "kind"
        ));
    }

    #[test]
    fn set_section_color_binds_a_local_section_row() {
        let (_dir, host, _kc) = open_host();
        let label: serde_json::Value = serde_json::from_str(
            &host
                .create_color_label_json("Doing".to_string(), "#34a853".to_string())
                .unwrap(),
        )
        .unwrap();
        let label_id = label["id"].as_str().unwrap().to_string();
        let list: serde_json::Value =
            serde_json::from_str(&host.create_task_list_json("Inbox".to_string()).unwrap())
                .unwrap();
        let list_id = list["id"].as_str().unwrap().to_string();
        let _ = host.task_lists_json().unwrap();
        let section: serde_json::Value = serde_json::from_str(
            &host
                .create_section_json(list_id.clone(), "Today".to_string(), 0, None)
                .unwrap(),
        )
        .unwrap();
        let section_id = section["id"].as_str().unwrap().to_string();

        host.set_section_color(section_id.clone(), list_id.clone(), Some(label_id.clone()))
            .unwrap();
        let sections: serde_json::Value =
            serde_json::from_str(&host.sections_json(list_id.clone()).unwrap()).unwrap();
        let bound = sections
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == serde_json::json!(section_id))
            .unwrap();
        assert_eq!(bound["color_label"], serde_json::json!(label_id));

        host.set_section_color(section_id.clone(), list_id.clone(), None)
            .unwrap();
        let sections: serde_json::Value =
            serde_json::from_str(&host.sections_json(list_id).unwrap()).unwrap();
        let cleared = sections
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == serde_json::json!(section_id))
            .unwrap();
        assert_eq!(cleared["color_label"], serde_json::Value::Null);
    }

    #[test]
    fn user_prefs_round_trip_locally() {
        let (_dir, host, _kc) = open_host();
        assert_eq!(host.get_user_pref("locale".to_string()).unwrap(), None);
        host.set_user_pref("locale".to_string(), "de".to_string())
            .unwrap();
        assert_eq!(
            host.get_user_pref("locale".to_string()).unwrap(),
            Some("de".to_string())
        );
        host.delete_user_pref("locale".to_string()).unwrap();
        assert_eq!(host.get_user_pref("locale".to_string()).unwrap(), None);
    }

    #[test]
    fn a_whitelisted_pref_syncs_across_devices_but_a_local_only_one_does_not() {
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        let dir_a = tempfile::tempdir().unwrap();
        let host_a = open_named(&dir_a, "a");
        let dir_b = tempfile::tempdir().unwrap();
        let host_b = open_named(&dir_b, "b");
        host_a.configure_sync_adapter_json(cfg.clone()).unwrap();
        host_b.configure_sync_adapter_json(cfg).unwrap();

        // `locale` is on the §19.2.1 sync whitelist; `sidebar.expansion` is not.
        host_a
            .set_user_pref("locale".to_string(), "de".to_string())
            .unwrap();
        host_a
            .set_user_pref("sidebar.expansion".to_string(), "open".to_string())
            .unwrap();
        wait_for_pending(&dir_a);
        host_a.sync_now_json().unwrap();
        host_b.sync_now_json().unwrap();

        assert_eq!(
            host_b.get_user_pref("locale".to_string()).unwrap(),
            Some("de".to_string()),
            "a whitelisted pref must reach the other device",
        );
        assert_eq!(
            host_b
                .get_user_pref("sidebar.expansion".to_string())
                .unwrap(),
            None,
            "a local-only pref must NOT propagate",
        );
    }

    #[test]
    fn enable_e2e_rejects_an_empty_passphrase() {
        let (_dir, host, _kc) = open_host();
        let err = host
            .enable_sync_encryption_json("   ".to_string())
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "passphrase"),
            "got: {err:?}"
        );
    }

    // Full webdav/ftp configure round-trips need a live server (test_connection
    // probes it), so they're an on-device / integration concern. Here we cover
    // the CI-safe validation branches that return BEFORE any network contact.

    #[test]
    fn webdav_configure_rejects_empty_url() {
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(r#"{"kind":"webdav","url":"  ","user":"a"}"#.to_string())
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidField { field, .. } if field == "url"),
            "got: {err:?}"
        );
    }

    #[test]
    fn ftp_configure_rejects_empty_host() {
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(r#"{"kind":"ftp","host":"","user":"a"}"#.to_string())
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidField { field, .. } if field == "host"),
            "got: {err:?}"
        );
    }

    #[test]
    fn ftp_configure_rejects_unknown_mode() {
        let (_dir, host, _kc) = open_host();
        // host + user are present, so validation reaches the mode check and
        // returns before opening the plugin / probing a server.
        let err = host
            .configure_sync_adapter_json(
                r#"{"kind":"ftp","host":"ftp.example.invalid","user":"a","mode":"bogus"}"#
                    .to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidField { field, .. } if field == "mode"),
            "got: {err:?}"
        );
    }

    #[test]
    fn unsupported_sync_kind_is_rejected() {
        // Every real kind is now configurable (local/webdav/ftp/dropbox/
        // googledrive/sftp); a bogus kind still errors on the field.
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(r#"{"kind":"icloud"}"#.to_string())
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidField { field, .. } if field == "kind"),
            "got: {err:?}"
        );
    }

    #[test]
    fn sftp_host_key_trust_round_trips_through_the_host() {
        // The pin store is the shared host-core verifier over user_prefs — no
        // network. trust → pinned → forget round-trips.
        let (_dir, host, _kc) = open_host();
        assert_eq!(
            host.pinned_sftp_host_key("nas:22".to_string()).unwrap(),
            None
        );
        host.trust_sftp_host_key("nas:22".to_string(), "SHA256:abc".to_string())
            .unwrap();
        assert_eq!(
            host.pinned_sftp_host_key("nas:22".to_string()).unwrap(),
            Some("SHA256:abc".to_string())
        );
        host.forget_sftp_host_key("nas:22".to_string()).unwrap();
        assert_eq!(
            host.pinned_sftp_host_key("nas:22".to_string()).unwrap(),
            None
        );
    }

    #[test]
    fn trust_sftp_host_key_rejects_empty_fingerprint() {
        let (_dir, host, _kc) = open_host();
        let err = host
            .trust_sftp_host_key("nas:22".to_string(), "  ".to_string())
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "fingerprint"),
            "got: {err:?}"
        );
    }

    #[test]
    fn configure_sftp_rejects_empty_host() {
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(
                r#"{"kind":"sftp","host":"","user":"u","path":"/p"}"#.to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "host"),
            "got: {err:?}"
        );
    }

    #[test]
    fn configure_sftp_key_auth_requires_a_key_path() {
        // key auth with no key_path fails fast (before any network), no keychain.
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(
                r#"{"kind":"sftp","host":"nas","user":"u","path":"/p","auth_method":"key"}"#
                    .to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "key_path"),
            "got: {err:?}"
        );
    }

    #[test]
    fn configure_sftp_password_auth_without_a_secret_is_auth_error() {
        // No password in the request + none stored → Auth, before any network.
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(
                r#"{"kind":"sftp","host":"nas","user":"u","path":"/p"}"#.to_string(),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::Auth { .. }), "got: {err:?}");
    }

    #[test]
    fn configure_sftp_without_a_trusted_pin_is_rejected() {
        // §19.5 backend guard: with a password supplied (so credential resolution
        // passes) but no pinned host key, configure must REJECT rather than
        // silently TOFU — before any network. Closes the MITM hole the UI-only
        // invariant left open.
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(
                r#"{"kind":"sftp","host":"nas","user":"u","path":"/p","password":"pw"}"#
                    .to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "pinned_fingerprint"),
            "got: {err:?}"
        );
    }

    #[test]
    fn sync_progress_driver_latches_after_three_failures_and_resets() {
        let driver = SyncProgressDriver::default();
        assert!(!driver.sustained());
        assert_eq!(driver.last_code(), None);

        driver.record_failure("network");
        driver.record_failure("network");
        assert!(!driver.sustained(), "two failures is below the threshold");
        assert_eq!(driver.last_code().as_deref(), Some("network"));

        driver.record_failure("io");
        assert!(
            driver.sustained(),
            "three consecutive failures latch sustained"
        );
        assert_eq!(driver.last_code().as_deref(), Some("io"));

        // A clean round clears both the streak and the latched code.
        driver.record_success();
        assert!(!driver.sustained());
        assert_eq!(driver.last_code(), None);
    }

    #[test]
    fn fresh_host_status_reports_no_sustained_failure() {
        let (_dir, host, _kc) = open_host();
        let status: serde_json::Value =
            serde_json::from_str(&host.sync_status_json().unwrap()).unwrap();
        // The decorated fields are present and clear on a fresh host.
        assert_eq!(status["sustained_failure"], serde_json::json!(false));
        assert_eq!(status["last_error_code"], serde_json::Value::Null);
    }

    #[test]
    fn task_lists_json_tags_local_lists_with_their_account() {
        let (_dir, host, _kc) = open_host();
        host.create_task_list_json("Mine".to_string()).unwrap();
        let lists: serde_json::Value =
            serde_json::from_str(&host.task_lists_json().unwrap()).unwrap();
        let arr = lists.as_array().unwrap();
        assert!(!arr.is_empty(), "the created list should be listed");
        // The TaskListRow shape carries account_id; with no external accounts
        // every list is local. (`id`/`name` stay top-level via the flatten.)
        assert!(
            arr.iter()
                .all(|l| l["account_id"] == serde_json::json!("local") && l["id"].is_string()),
            "every local list should be tagged account_id=local; got {lists}",
        );
        // Each row carries the adapter's task_capabilities so the UI can gate
        // affordances; the local store reports sections + nesting + recurrence.
        let caps = &arr[0]["task_capabilities"];
        assert_eq!(caps["sections"], serde_json::json!(true), "got {lists}");
        assert_eq!(caps["manageable_sections"], serde_json::json!(true));
        assert_eq!(caps["nested_projects"], serde_json::json!(true));
        assert_eq!(caps["task_recurrence"], serde_json::json!(true));
    }

    #[test]
    fn upcoming_reminders_surface_a_local_task_and_respect_the_horizon() {
        let (_dir, host, _kc) = open_host();
        let list = host.create_task_list_json("R".to_string()).unwrap();
        let list_id = serde_json::from_str::<serde_json::Value>(&list).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        // A task scheduled tomorrow at noon with a reminder 15 min before — its
        // trigger is comfortably in the future and within a 7-day horizon, but
        // outside a 30-minute one. (Local-time noon avoids DST edges.)
        let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let new_task = format!(
            r#"{{"title":"Ring me","description":null,"status":"open","priority":"medium","scheduled_date":"{tomorrow}","scheduled_time":"12:00:00","deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[{{"kind":{{"type":"relative","minutes_before":15}},"sound":null}}],"sound":null}}"#
        );
        let created = host.create_task_json(list_id, new_task).unwrap();
        let task_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // 7-day horizon includes it (tagged item_kind="task").
        let wide: serde_json::Value =
            serde_json::from_str(&host.upcoming_reminders_json(10_080).unwrap()).unwrap();
        assert!(
            wide.as_array().unwrap().iter().any(|r| {
                r["item_id"] == serde_json::json!(task_id)
                    && r["item_kind"] == serde_json::json!("task")
                    && r["trigger_at"].is_string()
            }),
            "the tomorrow reminder should be within a 7-day horizon; got: {wide}",
        );

        // 30-minute horizon excludes it (a tomorrow trigger is >24h out).
        let narrow: serde_json::Value =
            serde_json::from_str(&host.upcoming_reminders_json(30).unwrap()).unwrap();
        assert!(
            narrow
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["item_id"] != serde_json::json!(task_id)),
            "a tomorrow reminder must be outside a 30-minute horizon; got: {narrow}",
        );
    }

    #[test]
    fn begin_google_oauth_returns_an_authorize_url() {
        // End-to-end over the static embedding: the Host calls the bundled
        // Google plugin's interactive_auth(phase=authorize) — which only works
        // because register_static_with_auth carried the auth fn (8f313cb) — and
        // gets back a real consent URL. No network (the authorize phase is pure).
        let (_dir, host, _kc) = open_host();
        let args = r#"{"client_id":"abc.apps.googleusercontent.com","redirect_uri":"aperio://oauth-callback"}"#;
        let out = host
            .begin_oauth_json(
                "com.aperio.cal-adapter-google".to_string(),
                args.to_string(),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let url = v["authorize_url"].as_str().unwrap();
        assert!(url.contains("accounts.google.com"), "got: {url}");
        assert!(url.contains("abc.apps.googleusercontent.com"), "got: {url}");
        assert!(
            url.contains("aperio%3A%2F%2Foauth-callback"),
            "redirect must be in the URL: {url}"
        );
        assert_eq!(v["pkce_verifier"].as_str().unwrap().len(), 43);
        assert!(v["state"].as_str().is_some());
    }

    #[test]
    fn begin_microsoft_oauth_returns_an_authorize_url() {
        // Same static-embedding path as the Google case, for the Microsoft
        // plugin's authorize phase: a real v2.0 consent URL with the pinned
        // authority + the mobile redirect, no network.
        let (_dir, host, _kc) = open_host();
        let args = r#"{"client_id":"11111111-2222-3333-4444-555555555555","authority":"common","redirect_uri":"aperio://oauth-callback"}"#;
        let out = host
            .begin_oauth_json(
                "com.aperio.cal-adapter-microsoft-graph".to_string(),
                args.to_string(),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let url = v["authorize_url"].as_str().unwrap();
        assert!(
            url.contains("login.microsoftonline.com/common/oauth2/v2.0/authorize"),
            "got: {url}"
        );
        assert!(
            url.contains("11111111-2222-3333-4444-555555555555"),
            "got: {url}"
        );
        assert!(
            url.contains("aperio%3A%2F%2Foauth-callback"),
            "redirect must be in the URL: {url}"
        );
        assert_eq!(v["pkce_verifier"].as_str().unwrap().len(), 43);
        assert!(v["state"].as_str().is_some());
    }

    #[test]
    fn complete_oauth_unknown_plugin_errors_without_creating_an_account() {
        // The exchange runs FIRST and fails fast (PluginMissing → Auth) with no
        // network, so no orphan account row is left behind. (The happy path —
        // a real token exchange — is verified on-device.)
        let (_dir, host, _kc) = open_host();
        let before = host.accounts_json().unwrap();
        let req = r#"{"adapter_kind":"google","display_name":"G","config_json":"{}","client_id":"x","client_secret":"y","code":"c","pkce_verifier":"v","state":"s","returned_state":"s","redirect_uri":"aperio://oauth-callback"}"#;
        let err = host
            .complete_oauth_json("com.aperio.nonexistent-plugin".to_string(), req.to_string())
            .unwrap_err();
        assert!(matches!(err, StoreError::Auth { .. }), "got: {err:?}");
        assert_eq!(
            host.accounts_json().unwrap(),
            before,
            "a failed exchange must not create an account",
        );
    }

    #[test]
    fn begin_dropbox_oauth_returns_an_authorize_url() {
        // The sync-adapter plugins' interactive_auth is also wired into
        // host-plugins, so begin_oauth_json drives the Dropbox plugin's authorize
        // phase through the static embedding (no network).
        let (_dir, host, _kc) = open_host();
        let args = r#"{"client_id":"dbx-client","redirect_uri":"aperio://oauth-callback"}"#;
        let out = host
            .begin_oauth_json(
                "com.aperio.sync-adapter-dropbox".to_string(),
                args.to_string(),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let url = v["authorize_url"].as_str().unwrap();
        assert!(url.contains("dropbox.com/oauth2/authorize"), "got: {url}");
        assert!(
            url.contains("aperio%3A%2F%2Foauth-callback"),
            "redirect must be in the URL: {url}"
        );
        assert_eq!(v["pkce_verifier"].as_str().unwrap().len(), 43);
        assert!(v["state"].as_str().is_some());
    }

    #[test]
    fn begin_googledrive_oauth_returns_an_authorize_url() {
        let (_dir, host, _kc) = open_host();
        let args = r#"{"client_id":"gd-client","redirect_uri":"aperio://oauth-callback"}"#;
        let out = host
            .begin_oauth_json(
                "com.aperio.sync-adapter-googledrive".to_string(),
                args.to_string(),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let url = v["authorize_url"].as_str().unwrap();
        assert!(
            url.contains("accounts.google.com/o/oauth2/v2/auth"),
            "got: {url}"
        );
        assert!(
            url.contains("drive.file"),
            "drive scope must be present: {url}"
        );
        assert_eq!(v["pkce_verifier"].as_str().unwrap().len(), 43);
    }

    #[test]
    fn complete_sync_oauth_rejects_a_csrf_state_mismatch() {
        // The Dropbox plugin's exchange phase runs the CSRF check before the
        // network token POST, so a mismatched state aborts with no network (and
        // no refresh token is stored, since the store runs only after exchange).
        let (_dir, host, _kc) = open_host();
        let req = r#"{"client_id":"dbx","code":"c","pkce_verifier":"v","state":"AAAA","returned_state":"BBBB","redirect_uri":"aperio://oauth-callback"}"#;
        let err = host
            .complete_sync_oauth_json(
                "com.aperio.sync-adapter-dropbox".to_string(),
                req.to_string(),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::Auth { .. }), "got: {err:?}");
    }

    #[test]
    fn complete_sync_oauth_rejects_a_non_oauth_plugin() {
        let (_dir, host, _kc) = open_host();
        let req = r#"{"client_id":"x","code":"c","pkce_verifier":"v","state":"s","returned_state":"s","redirect_uri":"aperio://oauth-callback"}"#;
        let err = host
            .complete_sync_oauth_json(
                "com.aperio.sync-adapter-webdav".to_string(),
                req.to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "plugin_id"),
            "got: {err:?}"
        );
    }

    #[test]
    fn configure_dropbox_without_a_token_is_an_auth_error() {
        // No stored refresh token → configure fails fast at the keychain read,
        // before any network, prompting a sign-in.
        let (_dir, host, _kc) = open_host();
        let cfg = r#"{"kind":"dropbox","client_id":"dbx","path":"/Apps/Aperio"}"#;
        let err = host
            .configure_sync_adapter_json(cfg.to_string())
            .unwrap_err();
        assert!(matches!(err, StoreError::Auth { .. }), "got: {err:?}");
    }

    #[test]
    fn configure_googledrive_rejects_an_empty_client_secret() {
        let (_dir, host, _kc) = open_host();
        let cfg =
            r#"{"kind":"googledrive","client_id":"gd","client_secret":"","folder_name":"Aperio"}"#;
        let err = host
            .configure_sync_adapter_json(cfg.to_string())
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "client_secret"),
            "got: {err:?}"
        );
    }

    #[test]
    fn discover_json_validates_at_the_plugin_without_network() {
        // The EWS plugin's discover handler rejects an empty email BEFORE it
        // builds the HTTP client, so this exercises the full static chain
        // (register! discover arm → EWS plugin → cal-ffi) with no network and
        // surfaces the plugin's actionable message as a Protocol error.
        let (_dir, host, _kc) = open_host();
        let err = host
            .discover_json(
                "com.aperio.cal-adapter-ews".to_string(),
                r#"{"email":"","password":"x"}"#.to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Protocol { ref detail } if detail.contains("email")),
            "got: {err:?}"
        );
    }

    #[test]
    fn discover_json_unknown_plugin_is_unsupported() {
        let (_dir, host, _kc) = open_host();
        let err = host
            .discover_json(
                "com.aperio.nonexistent-plugin".to_string(),
                r#"{"email":"a@b.com","password":"x"}"#.to_string(),
            )
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Unsupported { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn complete_oauth_rejects_a_csrf_state_mismatch_without_creating_an_account() {
        // The plugin's exchange phase runs the CSRF (state) check BEFORE the
        // network token POST, so a mismatched issued-vs-returned state aborts with
        // no network and no orphan account row. Guards the fail-closed check.
        let (_dir, host, _kc) = open_host();
        let before = host.accounts_json().unwrap();
        let req = r#"{"adapter_kind":"google","display_name":"G","config_json":"{}","client_id":"x","client_secret":"y","code":"c","pkce_verifier":"v","state":"AAAA","returned_state":"BBBB","redirect_uri":"aperio://oauth-callback"}"#;
        let err = host
            .complete_oauth_json("com.aperio.cal-adapter-google".to_string(), req.to_string())
            .unwrap_err();
        assert!(matches!(err, StoreError::Auth { .. }), "got: {err:?}");
        assert!(
            err.to_string().contains("CSRF") || err.to_string().contains("state mismatch"),
            "expected a CSRF state-mismatch error, got: {err}"
        );
        assert_eq!(
            host.accounts_json().unwrap(),
            before,
            "a CSRF-rejected exchange must not create an account",
        );
    }

    #[test]
    fn complete_oauth_rejects_an_empty_display_name() {
        // The host enforces the desktop's non-empty guards regardless of caller.
        let (_dir, host, _kc) = open_host();
        let req = r#"{"adapter_kind":"google","display_name":"  ","config_json":"{}","client_id":"x","client_secret":"y","code":"c","pkce_verifier":"v","state":"s","returned_state":"s","redirect_uri":"aperio://oauth-callback"}"#;
        let err = host
            .complete_oauth_json("com.aperio.cal-adapter-google".to_string(), req.to_string())
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidField { ref field, .. } if field == "display_name"),
            "got: {err:?}"
        );
    }

    #[test]
    fn birthday_calendar_synthesises_a_local_contacts_birthday() {
        let (_dir, host, _kc) = open_host();
        // The seeded local address book (migration 0007).
        let lists: serde_json::Value =
            serde_json::from_str(&host.contact_lists_json().unwrap()).unwrap();
        let list_id = lists.as_array().unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        // A contact with a June birthday (so the existing covering_range hits it).
        let new_contact = r#"{"display_name":"Ada Lovelace","given_name":null,"family_name":null,"organization":null,"emails":[],"phone_numbers":[],"birthday":"1990-06-15","notes":null,"addresses":[],"members":null,"photo":null}"#;
        host.create_contact_json(list_id.clone(), new_contact.to_string())
            .unwrap();

        // list_calendars now surfaces a synthetic, read-only birthday calendar.
        let cals: serde_json::Value =
            serde_json::from_str(&host.list_calendars_json().unwrap()).unwrap();
        let bday = cals
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["id"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("aperio-birthdays:"))
            })
            .expect("a birthday calendar should appear once a contact has a birthday");
        assert_eq!(bday["read_only"], serde_json::json!(true));
        let bday_id = bday["id"].as_str().unwrap().to_string();

        // get_events synthesises the all-day birthday occurrence in range.
        let events: serde_json::Value =
            serde_json::from_str(&host.get_events_json(covering_range(&bday_id)).unwrap()).unwrap();
        let arr = events.as_array().unwrap();
        assert_eq!(
            arr.len(),
            1,
            "one birthday occurrence in June; got: {events}"
        );
        assert_eq!(arr[0]["title"], serde_json::json!("Ada Lovelace"));
        assert_eq!(arr[0]["all_day"], serde_json::json!(true));
    }

    #[test]
    fn contacts_round_trip_through_the_local_address_book() {
        let (_dir, host, _kc) = open_host();
        // Migration 0007 seeds the default local address book.
        let lists: serde_json::Value =
            serde_json::from_str(&host.contact_lists_json().unwrap()).unwrap();
        let arr = lists.as_array().unwrap();
        assert!(!arr.is_empty(), "the seeded local address book should list");
        let list_id = arr[0]["id"].as_str().unwrap().to_string();
        assert_eq!(arr[0]["account_id"], serde_json::json!("local"));

        // Create a contact (every NewContact field supplied so serde can't trip
        // on a missing one regardless of per-field defaults).
        let new_contact = r#"{"display_name":"Ada Lovelace","given_name":"Ada","family_name":"Lovelace","organization":null,"emails":["ada@example.com"],"phone_numbers":["+44 20 7946 0000"],"birthday":null,"notes":null,"addresses":[],"members":null,"photo":null}"#;
        let created = host
            .create_contact_json(list_id.clone(), new_contact.to_string())
            .unwrap();
        let mut contact: serde_json::Value = serde_json::from_str(&created).unwrap();
        let contact_id = contact["id"].as_str().unwrap().to_string();
        assert_eq!(contact["display_name"], "Ada Lovelace");

        // It shows up in the list's contacts.
        let listed: serde_json::Value =
            serde_json::from_str(&host.contacts_json(list_id.clone()).unwrap()).unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == serde_json::json!(contact_id)),
            "the created contact should be listed; got: {listed}",
        );

        // Update (full read-modify-write round-trip).
        contact["display_name"] = serde_json::json!("Augusta Ada King");
        let updated: serde_json::Value =
            serde_json::from_str(&host.update_contact_json(contact.to_string()).unwrap()).unwrap();
        assert_eq!(updated["display_name"], "Augusta Ada King");

        // Delete (routed by the owning list).
        host.delete_contact(contact_id.clone(), Some(list_id.clone()))
            .unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&host.contacts_json(list_id).unwrap()).unwrap();
        assert!(
            after
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["id"] != serde_json::json!(contact_id)),
            "the contact should be gone after delete; got: {after}",
        );
    }
}
