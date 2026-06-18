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

use cal_adapter_local::LocalAdapter;
use cal_core::{
    Calendar, CalendarFeature, ColorLabelId, ContactList, ContactsFeature, DateRange, Event,
    NewEvent, TaskList, TasksFeature,
};
use host_core::accounts::{AccountsRepo, AdapterKind};
use host_core::event_log::OnboardingService;
use host_core::registry::{AdapterRegistry, LOCAL_ID};
use host_core::sync::build_orchestrator;
use host_core::user_prefs::UserPrefsRepo;
use host_core::DbHandle;
use plugin_core::shim::FfiSyncAdapter;
use plugin_core::PluginManager;
use sync_core::{AccountPayload, EventPayload, IdPayload, SyncAdapter, SyncError, SyncEvent};
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
/// Plugin ids of the statically-embedded sync adapters this host configures.
/// SFTP (needs the §19.5 host-key trust flow) + the OAuth kinds (Dropbox /
/// Google Drive) follow in their own phases.
const PLUGIN_ID_SYNC_LOCAL: &str = "com.aperio.sync-adapter-local";
const PLUGIN_ID_WEBDAV: &str = "com.aperio.sync-adapter-webdav";
const PLUGIN_ID_FTP: &str = "com.aperio.sync-adapter-ftp";

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

/// Error for a write attempted against an external task list. Reads route to the
/// provider, but external task WRITES on mobile are a later phase, so a clear
/// `Unsupported` beats a confusing local NotFound.
fn external_tasks_readonly() -> StoreError {
    StoreError::Unsupported {
        detail: "editing tasks from external accounts on mobile is not supported yet".to_string(),
    }
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
/// SFTP (host-key trust flow) + the OAuth kinds (Dropbox / Google Drive) follow.
#[derive(serde::Deserialize)]
struct ConfigureSyncRequest {
    kind: String,
    /// `local` filesystem path / `ftp` remote path.
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
    // Held for the lifetime of the host (snapshot consume/produce + meta
    // heartbeats); the onboarding command surface is a later phase.
    _onboarding: Arc<OnboardingService>,
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

/// A task list enriched with its owning `account_id` — the desktop `TaskListRow`
/// wire shape. `task_capabilities` is intentionally omitted here (the mobile UI
/// doesn't consume it yet → the shared `TaskList` type has it optional, and
/// cal-core's default applies); it joins when the capabilities surface is ported.
#[derive(serde::Serialize)]
struct TaskListRow {
    #[serde(flatten)]
    inner: TaskList,
    account_id: String,
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
/// sync unconfigured until the user re-configures from the Sync screen. Only the
/// kinds this host can configure are restored (`local` / `webdav` / `ftp`).
fn restore_adapter_from_prefs(
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
            if let Some(adapter) =
                restore_adapter_from_prefs(&prefs, &plugin_manager, secret_store.as_ref())
            {
                graph.orchestrator.configure(adapter);
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
            _onboarding: graph.onboarding,
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
        let rows = self.runtime.block_on(async {
            let local = self.adapter.list_calendars().await.map_err(map_store_err)?;
            for c in &local {
                self.registry.note_calendar_route(&c.id, LOCAL_ID);
            }
            // `list_external_calendars` stamps external routes internally and
            // swallows per-adapter errors so one dead account can't blank the
            // whole list.
            let external = self.registry.list_external_calendars().await;

            let mut out: Vec<CalendarRow> = Vec::with_capacity(local.len() + external.len());
            for c in local {
                out.push(CalendarRow::new(c, LOCAL_ID.to_string()));
            }
            for c in external {
                let acct = self
                    .registry
                    .account_for_calendar(&c.id)
                    .unwrap_or_else(|| LOCAL_ID.to_string());
                out.push(CalendarRow::new(c, acct));
            }
            Ok::<_, StoreError>(out)
        })?;
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
    /// live, exactly as a cache-cold desktop first read. Birthday calendars are
    /// deferred (desktop-only) — a birthday id routes to empty.
    ///
    /// The local adapter currently returns rows whose stored start/end
    /// intersect the range (RRULE occurrence expansion is its own later phase),
    /// so a recurring master is returned only when its stored span overlaps.
    pub fn get_events_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: EventRangeRequest = from_json("request", &request_json)?;
        let range = DateRange::new(req.start, req.end);
        let events = self.runtime.block_on(async {
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

        let mut rows: Vec<TaskListRow> = Vec::with_capacity(local.len() + external.len());
        for l in local {
            rows.push(TaskListRow {
                inner: l,
                account_id: LOCAL_ID.to_string(),
            });
        }
        for l in external {
            let account_id = self
                .registry
                .account_for_task_list(&l.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            rows.push(TaskListRow {
                inner: l,
                account_id,
            });
        }
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
            return Err(external_tasks_readonly());
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
                let sections = self
                    .runtime
                    .block_on(async { ext.list_sections(&list_id).await })
                    .map_err(map_store_err)?;
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
        }
        Ok(value.to_string())
    }

    /// Configure the sync adapter from a JSON request. Handles the `local`
    /// (filesystem path), `webdav` (URL + user + password), and `ftp`
    /// (host/port/user/path/mode + password) kinds: open the matching
    /// statically-embedded sync plugin, probe it (`test_connection`), make it
    /// the orchestrator's active adapter, and persist the choice under the
    /// `sync.adapter.*` prefs (device-local; the is_synced_key allowlist
    /// excludes them, so they never propagate). The credential goes to the
    /// keychain via the platform `SecretStore`; an omitted/empty password reuses
    /// the stored one. SFTP (host-key trust flow) + the E2E `wrap_if_encrypted`
    /// branch + the OAuth kinds (Dropbox / Google Drive) follow.
    pub fn configure_sync_adapter_json(&self, config_json: String) -> Result<(), StoreError> {
        let req: ConfigureSyncRequest = from_json("sync config", &config_json)?;
        match req.kind.as_str() {
            "local" => {
                let path = req.path.unwrap_or_default();
                let path = path.trim();
                if path.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "path".to_string(),
                        detail: "sync path must not be empty".to_string(),
                    });
                }
                let cfg = serde_json::json!({ "remote_root": path }).to_string();
                let adapter = open_sync_plugin(&self.plugin_manager, PLUGIN_ID_SYNC_LOCAL, cfg)?;
                // Probe before keeping it active so a bad path fails here.
                self.runtime
                    .block_on(async { adapter.test_connection().await })
                    .map_err(sync_err)?;
                self.orchestrator.configure(adapter);
                let shared = self.db.shared();
                let prefs = UserPrefsRepo::new(&shared);
                prefs
                    .set(PREF_ADAPTER_KIND, "local")
                    .map_err(|e| StoreError::Storage {
                        detail: e.to_string(),
                    })?;
                prefs
                    .set(PREF_LOCAL_PATH, path)
                    .map_err(|e| StoreError::Storage {
                        detail: e.to_string(),
                    })?;
                Ok(())
            }
            "webdav" => {
                let url = req.url.unwrap_or_default();
                let url = url.trim();
                if url.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "url".to_string(),
                        detail: "WebDAV URL must not be empty".to_string(),
                    });
                }
                let user = req.user.unwrap_or_default();
                let user = user.trim();
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
                let adapter = open_sync_plugin(&self.plugin_manager, PLUGIN_ID_WEBDAV, cfg)?;
                // Probe before keeping it active so bad creds / a bad URL fail
                // here rather than on the first silent sync round.
                self.runtime
                    .block_on(async { adapter.test_connection().await })
                    .map_err(sync_err)?;
                self.orchestrator.configure(adapter);
                let shared = self.db.shared();
                let prefs = UserPrefsRepo::new(&shared);
                prefs
                    .set(PREF_ADAPTER_KIND, "webdav")
                    .map_err(storage_err)?;
                prefs.set(PREF_WEBDAV_URL, url).map_err(storage_err)?;
                prefs.set(PREF_WEBDAV_USER, user).map_err(storage_err)?;
                // Only overwrite the keychain when the request carries a
                // non-empty password — URL/user edits keep the prior secret.
                if let Some(pw) = req.password.as_deref().map(str::trim) {
                    if !pw.is_empty() {
                        self.secret_store
                            .store(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password, pw)
                            .map_err(storage_err)?;
                    }
                }
                Ok(())
            }
            "ftp" => {
                let host = req.host.unwrap_or_default();
                let host = host.trim();
                if host.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "host".to_string(),
                        detail: "FTP host must not be empty".to_string(),
                    });
                }
                let user = req.user.unwrap_or_default();
                let user = user.trim();
                if user.is_empty() {
                    return Err(StoreError::InvalidField {
                        field: "user".to_string(),
                        detail: "FTP user must not be empty".to_string(),
                    });
                }
                let port = req.port.unwrap_or(21);
                let path = req.path.unwrap_or_default();
                let path = path.trim();
                let mode = req.mode.unwrap_or_else(|| "explicit".to_string());
                let mode = mode.trim();
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
                let adapter = open_sync_plugin(&self.plugin_manager, PLUGIN_ID_FTP, cfg)?;
                self.runtime
                    .block_on(async { adapter.test_connection().await })
                    .map_err(sync_err)?;
                self.orchestrator.configure(adapter);
                let shared = self.db.shared();
                let prefs = UserPrefsRepo::new(&shared);
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
                Ok(())
            }
            other => Err(StoreError::InvalidField {
                field: "kind".to_string(),
                detail: format!(
                    "sync adapter kind '{other}' is not supported yet \
                     (local, webdav, ftp)"
                ),
            }),
        }
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
        let mut rows: Vec<ContactListRow> = Vec::with_capacity(local.len() + external.len());
        for l in local {
            rows.push(ContactListRow {
                inner: l,
                account_id: LOCAL_ID.to_string(),
            });
        }
        for l in external {
            let account_id = self
                .registry
                .account_for_contact_list(&l.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            rows.push(ContactListRow {
                inner: l,
                account_id,
            });
        }
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
        let (_dir, host, _kc) = open_host();
        let err = host
            .configure_sync_adapter_json(r#"{"kind":"dropbox"}"#.to_string())
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidField { field, .. } if field == "kind"),
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
