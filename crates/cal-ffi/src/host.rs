//! The mobile `Host` — the on-device counterpart to the desktop
//! `src-tauri` backend, assembled from the shared `host-core` crate.
//!
//! Where [`crate::LocalStore`] serves only the local SQLite task store,
//! `Host` owns the full account + adapter surface: it opens the same
//! migrated database, statically links + registers all 14 bundled
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

use adapter_device_calendar::{DeviceAdapter, DeviceCalendarProvider};
use adapter_local::{prepare_fts_query, LocalAdapter, SearchFilters};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use cal_core::{
    Calendar, CalendarFeature, ColorLabelId, ContactList, ContactsFeature, DateRange, Event,
    NewEvent, TaskList, TasksFeature,
};
use host_core::accounts::{AccountsRepo, AdapterKind};
use host_core::cache::{
    event_self_warm_needed, has_snapshot, is_stale, refresh_contacts, refresh_events,
    refresh_sections, refresh_tasks, spawn_item_refresh, spawn_refresh, CacheObserver,
    CacheRefresher, CacheStore, CacheUpdatedPayload, RefreshCoordinator, SyncScope, SWR_TTL_SECS,
};
use host_core::conflicts::{
    ConflictKind, ConflictRecord, ConflictsError, ConflictsRepo, ResolutionChoice,
};
use host_core::contact_sync::{ContactSyncCore, ContactSyncObserver, ContactsSyncedPayload};
use host_core::db::SharedConn;
use host_core::event_groups::{EventGroupsError, EventGroupsRepo, NewMember, Removal, Ungrouped};
use host_core::event_log::OnboardingService;
use host_core::meetings::{
    attendee_addresses, should_provider_announce_removal, should_provider_notify, MeetingsRepo,
};
use host_core::overrides::{
    apply_color_to_calendars, apply_color_to_contact_lists, apply_color_to_events,
    apply_color_to_sections, apply_color_to_task_lists, apply_to_calendars, apply_to_task_lists,
    ContainerKind, OverridesError, OverridesRepo,
};
use host_core::registry::{AdapterRegistry, LOCAL_ID};
use host_core::sftp_host_keys::UserPrefsHostKeyVerifier;
use host_core::sync::build_orchestrator;
use host_core::sync_log::{SyncLogCounters, SyncLogRepo, SyncTrigger};
use host_core::user_prefs::UserPrefsRepo;
use host_core::DbHandle;
use plugin_core::manifest::{RecurrenceCapabilities, TaskCapabilities};
use plugin_core::shim::FfiSyncAdapter;
use plugin_core::PluginManager;
use sync_core::{
    derive_key, fresh_data_key, resolve_data_key, wrap_key, AccountPayload, EncryptingAdapter,
    EncryptionParams, EventPayload, IdPayload, SettingsPayload, SyncAdapter, SyncError, SyncEvent,
    KEY_LEN,
};
use sync_engine::{EventLogWriter, SecretError, SecretSlot, SecretStore, SyncOrchestrator};
use vc_core::NewMeeting;

/// Non-secret summary of the configured sync target — its kind plus a human
/// "detail" string (user@url / host:port/path / folder), for the Settings
/// "connected" card. Serializes to `null` (Option::None) when nothing is
/// configured. Mirrors the desktop `SyncAdapterSummary`.
#[derive(serde::Serialize)]
struct SyncAdapterSummary {
    kind: String,
    detail: String,
    /// The account row this device syncs through, when it syncs through one.
    ///
    /// `None` on a device still reading the legacy `sync.adapter.*`
    /// preferences. The sync settings need the ID rather than the name: they
    /// render a list of the accounts that could hold the dataset and have to
    /// mark the one that does — and two accounts may legitimately carry the
    /// same display name.
    account_id: Option<String>,
}
/// Plugin ids of the statically-embedded sync adapters this host configures.
/// The sync-target names, owned by `host_core::sync_target` so this host and
/// the other one cannot drift apart. Aliased where the local spelling differed,
/// which keeps this commit to the declarations themselves.
use host_core::sync_target::{
    is_unconfigured, PLUGIN_ID_DROPBOX, PLUGIN_ID_FTP, PLUGIN_ID_GOOGLEDRIVE, PLUGIN_ID_SFTP,
    PLUGIN_ID_WEBDAV, PREF_ADAPTER_KIND, PREF_DROPBOX_PATH, PREF_FTP_HOST, PREF_FTP_PATH,
    PREF_FTP_PORT, PREF_FTP_USER, PREF_GOOGLEDRIVE_FOLDER_NAME, PREF_LOCAL_PATH, PREF_SFTP_HOST,
    PREF_SFTP_PATH, PREF_SFTP_PORT, PREF_SFTP_USER, PREF_WEBDAV_URL, PREF_WEBDAV_USER,
    SECRET_ACCOUNT_DROPBOX as DROPBOX_SECRET_ACCOUNT, SECRET_ACCOUNT_E2E as E2E_SECRET_ACCOUNT,
};
// The four per-kind credential pseudo-accounts are gone from this file. Nothing
// here reads a credential by its legacy address any more: `stored_secret` knows
// where it lives, and only the OAuth completion still WRITES to one — it runs
// before there is an account row to write to.

// Imported below with the other five rather than declared here. It was the
// one this host still spelled out for itself — the same string today, and a
// second place for it to stop being the same tomorrow.
use host_core::sync_target::PLUGIN_ID_LOCAL as PLUGIN_ID_SYNC_LOCAL;

/// Why a sync target could not be built from a form, in a shape the phone can
/// act on.
///
/// The host-key refusal keeps a code, because its repair is a GESTURE: the
/// fingerprint has to be shown and accepted, and a screen that only printed the
/// sentence would leave the user with nothing to press. It rides the same
/// `Sync` variant the engine's own failures use, so the mobile frontend maps it
/// through one function rather than two.
///
/// Everything else carries the error's own text — it names the field or repeats
/// the plugin's complaint, which is more than a code could say.
fn connect_err(e: host_core::sync_target::ConnectError) -> StoreError {
    use host_core::sync_target::ConnectError as E;
    match &e {
        E::HostKeyNotTrusted { .. } => StoreError::Sync {
            code: "host_key_not_trusted".to_string(),
            detail: e.to_string(),
        },
        _ => StoreError::Storage {
            detail: e.to_string(),
        },
    }
}

fn sync_err(e: SyncError) -> StoreError {
    StoreError::Sync {
        code: e.code().to_string(),
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

/// The synced, default-ON "show cancelled events" setting
/// (`view.showCancelledEvents`). Only an explicit `"false"` hides them; any
/// other value (including unset) shows them, for Outlook consistency. The
/// desktop twin lives in the frontend `ViewState` filter.
fn show_cancelled_events(db: &SharedConn) -> bool {
    UserPrefsRepo::new(db)
        .get("view.showCancelledEvents")
        .ok()
        .flatten()
        .as_deref()
        != Some("false")
}

/// Parse a wire `AttendeeStatus` (kebab-case: "accepted" / "tentative" /
/// "declined" / "needs-action") into the core enum, via its serde rename so the
/// mapping never drifts from the type.
fn parse_attendee_status(s: &str) -> Result<cal_core::AttendeeStatus, StoreError> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(|_| {
        StoreError::InvalidField {
            field: "status".to_string(),
            detail: format!("unknown attendee status '{s}'"),
        }
    })
}

/// Map the shared custom-sound importer's error onto the bridge `StoreError`:
/// a rejected format/size is an `InvalidField`, an IO failure is `Storage`.
fn map_import_err(err: host_core::sound_assets::ImportSoundError) -> StoreError {
    use host_core::sound_assets::ImportSoundError as E;
    let detail = err.to_string();
    match err {
        E::UnsupportedFormat(_) | E::TooLarge { .. } => StoreError::InvalidField {
            field: "sound".to_string(),
            detail,
        },
        E::Io(_) => StoreError::Storage { detail },
    }
}

/// Parse a wire `MemberRight` (snake_case: "read" / "write" / "admin") into the
/// core enum, via its serde rename.
fn parse_member_right(s: &str) -> Result<cal_core::MemberRight, StoreError> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(|_| {
        StoreError::InvalidField {
            field: "right".to_string(),
            detail: format!("unknown member right '{s}'"),
        }
    })
}

/// Map a `ConflictsRepo` failure into the bridge's `StoreError`: a missing row
/// is `NotFound`, a bad kind/resolution is an `InvalidField`, the rest SQLite.
fn map_conflicts_err(e: ConflictsError) -> StoreError {
    match e {
        ConflictsError::NotFound(_) => StoreError::NotFound,
        ConflictsError::Sqlite(err) => StoreError::Storage {
            detail: err.to_string(),
        },
        other => StoreError::InvalidField {
            field: "conflict".to_string(),
            detail: other.to_string(),
        },
    }
}

/// Generic single-field patch via serde — round-trips the row through `Value`,
/// overwrites the one field with the (JSON-decoded) remote value, deserialises
/// back. Mirrors the desktop `patch_field` so every `apply_take_remote` branch
/// stays boilerplate-free.
fn patch_field<T>(row: &mut T, field: &str, value: &serde_json::Value) -> Result<(), StoreError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut serialised = serde_json::to_value(&*row).map_err(|err| StoreError::Storage {
        detail: format!("serialise row for patch: {err}"),
    })?;
    if let Some(obj) = serialised.as_object_mut() {
        obj.insert(field.to_string(), value.clone());
    }
    *row = serde_json::from_value(serialised).map_err(|err| StoreError::Storage {
        detail: format!("deserialise patched row: {err}"),
    })?;
    Ok(())
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

/// Append an `account.*` event unless this adapter's accounts stay on the
/// device that made them. Returns whether the event was actually appended.
///
/// Every emit site goes through here so the rule is stated once; the reasoning
/// is in [`host_core::accounts::travels_between_devices`]. The desktop twin
/// sits next to its own `account_payload`.
fn append_account_event(
    writer: &EventLogWriter,
    plugin_manager: &PluginManager,
    adapter_kind: &str,
    event: SyncEvent,
) -> bool {
    if !host_core::accounts::travels_between_devices(plugin_manager, adapter_kind) {
        return false;
    }
    writer.append(event);
    true
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
/// `recurrence_capabilities` is stamped from the owning adapter's plugin
/// manifest (full RFC-5545 for the local store / any account whose plugin can't
/// be resolved), so the event editor greys out recurrence options the backend
/// can't store rather than offering one it silently drops. Mirrors the desktop
/// `CalendarRow`.
#[derive(serde::Serialize)]
struct CalendarRow {
    #[serde(flatten)]
    inner: Calendar,
    account_id: String,
    recurrence_capabilities: RecurrenceCapabilities,
}

impl CalendarRow {
    fn new(
        inner: Calendar,
        account_id: String,
        recurrence_capabilities: RecurrenceCapabilities,
    ) -> Self {
        Self {
            inner,
            account_id,
            recurrence_capabilities,
        }
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

/// Free/busy lookup request — the desktop `query_free_busy` payload.
/// `range_start`/`range_end` are RFC-3339 UTC instants.
#[derive(serde::Deserialize)]
struct FreeBusyRequest {
    calendar_id: String,
    emails: Vec<String>,
    range_start: chrono::DateTime<chrono::Utc>,
    range_end: chrono::DateTime<chrono::Utc>,
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
    /// The external-adapter snapshot cache (CACHE-1/2). External reads serve the
    /// cached snapshot instantly and queue a deduplicated background refresh —
    /// the desktop SWR behaviour, now shared via host-core.
    cache: Arc<CacheStore>,
    /// Dedups concurrent background refreshes of the same container.
    coord: Arc<RefreshCoordinator>,
    /// Where a finished background refresh / warm pass reports progress so the
    /// UI can re-read. Swappable: a no-op until the JS layer registers its bridge
    /// via [`Host::set_cache_observer`]; the cache populates regardless.
    cache_observer: Arc<BridgeCacheObserver>,
    /// Drives manual + on-foreground warm passes (the mobile stand-in for the
    /// desktop periodic loop — constructed without `start_periodic`).
    cache_refresher: Arc<host_core::cache::CacheRefresher>,
    /// The contact-sync core (the same one the desktop wraps in a tokio loop).
    /// Mobile drives `run_sync` from foreground triggers / the manual button —
    /// no app-start loop. Shares the bootstrapped `registry`.
    contact_sync: Arc<ContactSyncCore>,
    /// Where a finished contact-sync pass reports so the UI can re-read + update
    /// the "last synced" footer. A no-op until the JS layer registers its bridge
    /// via [`Host::set_contact_sync_observer`]; passes run regardless.
    contact_sync_observer: Arc<BridgeContactSyncObserver>,
    /// The native device calendar/reminder provider (iOS EventKit; Android has
    /// none yet). `None` until the native module installs it via
    /// [`Host::set_device_event_store`] right after open. The device-calendar
    /// adapter can't be added or registered without it, so adding such an
    /// account fails cleanly on a platform that never sets it.
    device_provider: std::sync::RwLock<Option<Arc<dyn DeviceCalendarProvider>>>,
    /// `<data_dir>/assets/sounds/` — the content-addressed custom-sound store
    /// (`<sha256>.<ext>`), the desktop `SoundsDir` twin. Imports write here, the
    /// sync round push/fetches it (DesktopSyncRoundHooks, already wired via
    /// build_orchestrator), and the reminder scheduler resolves a Custom sound's
    /// hash to a file here for the native Android notification channel.
    sounds_dir: std::path::PathBuf,
}

/// Foreign-side sink for cache-refresh progress (the mobile analogue of the
/// desktop Tauri `cache-updated` / `cache-refresh-status` events). The JS layer
/// implements this and registers it via [`Host::set_cache_observer`]; the
/// payloads are JSON strings carrying the same shape as the desktop events, so
/// the RN side can invalidate the matching view + render a refresh status.
#[uniffi::export(with_foreign)]
pub trait CacheObserverBridge: Send + Sync {
    /// One container's snapshot changed (a background refresh wrote fresh data)
    /// — the UI should re-read that view. `payload_json` is a `CacheUpdatedPayload`
    /// (`{scope, account_id, container_id}`).
    fn cache_updated(&self, payload_json: String);
    /// A warm pass changed its running / last-completed state. `status_json` is
    /// a `CacheRefreshStatus` (`{refreshing, last_refreshed_at}`).
    fn refresh_status(&self, status_json: String);
}

/// Adapts a foreign [`CacheObserverBridge`] to the engine-side [`CacheObserver`]
/// the SWR helpers + the refresher call. The bridge is installed AFTER
/// construction (the JS layer registers it once it's ready), so it's held behind
/// an `RwLock<Option<…>>`: the Host hands the same observer to background spawns
/// made before registration, and every notification is a no-op until a bridge is
/// set. Mirrors [`BridgeSecretStore`].
struct BridgeCacheObserver {
    bridge: std::sync::RwLock<Option<Arc<dyn CacheObserverBridge>>>,
}

impl BridgeCacheObserver {
    fn new() -> Self {
        Self {
            bridge: std::sync::RwLock::new(None),
        }
    }

    fn set(&self, bridge: Arc<dyn CacheObserverBridge>) {
        if let Ok(mut guard) = self.bridge.write() {
            *guard = Some(bridge);
        }
    }
}

impl CacheObserver for BridgeCacheObserver {
    fn cache_updated(&self, payload: &host_core::cache::CacheUpdatedPayload) {
        if let Ok(guard) = self.bridge.read() {
            if let Some(bridge) = guard.as_ref() {
                if let Ok(json) = serde_json::to_string(payload) {
                    bridge.cache_updated(json);
                }
            }
        }
    }

    fn refresh_status(&self, status: &host_core::cache::CacheRefreshStatus) {
        if let Ok(guard) = self.bridge.read() {
            if let Some(bridge) = guard.as_ref() {
                if let Ok(json) = serde_json::to_string(status) {
                    bridge.refresh_status(json);
                }
            }
        }
    }
}

/// Foreign-side sink for the "a contact-sync pass finished" broadcast (the
/// mobile analogue of the desktop Tauri `contacts-synced` event). The JS layer
/// implements this and registers it via [`Host::set_contact_sync_observer`]; the
/// payload is a JSON string with the same shape the desktop event carries, so
/// the RN side can update the "last synced" footer + re-read the contact views.
#[uniffi::export(with_foreign)]
pub trait ContactSyncObserverBridge: Send + Sync {
    /// A contact-sync pass finished. `payload_json` is a `ContactsSyncedPayload`
    /// (`{last_synced_at, succeeded_accounts, failed_accounts}`).
    fn contacts_synced(&self, payload_json: String);
}

/// Adapts a foreign [`ContactSyncObserverBridge`] to the engine-side
/// [`ContactSyncObserver`] the core calls. Installed AFTER construction (the JS
/// layer registers it once ready), so it's held behind an `RwLock<Option<…>>`
/// and every notification is a no-op until a bridge is set. Mirrors
/// [`BridgeCacheObserver`].
struct BridgeContactSyncObserver {
    bridge: std::sync::RwLock<Option<Arc<dyn ContactSyncObserverBridge>>>,
}

impl BridgeContactSyncObserver {
    fn new() -> Self {
        Self {
            bridge: std::sync::RwLock::new(None),
        }
    }

    fn set(&self, bridge: Arc<dyn ContactSyncObserverBridge>) {
        if let Ok(mut guard) = self.bridge.write() {
            *guard = Some(bridge);
        }
    }
}

impl ContactSyncObserver for BridgeContactSyncObserver {
    fn contacts_synced(&self, payload: &ContactsSyncedPayload) {
        if let Ok(guard) = self.bridge.read() {
            if let Some(bridge) = guard.as_ref() {
                if let Ok(json) = serde_json::to_string(payload) {
                    bridge.contacts_synced(json);
                }
            }
        }
    }
}

/// Error from the native device calendar / reminder store, surfaced across the
/// foreign boundary. Mirrors [`KeychainError`]'s shape; the engine maps it to a
/// [`cal_core::Error`] so a device failure reads like any other adapter's.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum DeviceCalError {
    /// The OS permission prompt was denied (or access was revoked).
    #[error("device calendar permission denied")]
    PermissionDenied,
    /// No device calendar/reminder store on this platform (e.g. Android has no
    /// system reminders app) or the store is otherwise unavailable.
    #[error("device calendar/reminders unavailable")]
    Unavailable,
    /// The native store call failed.
    #[error("device store error: {detail}")]
    Backend { detail: String },
}

/// Foreign-side bridge to the native device calendar + reminders store (iOS
/// EventKit `EKEvent`/`EKReminder`; Android `CalendarProvider` later). The
/// mobile native module implements it (Swift `IosDeviceEventStore`) and installs
/// it via [`Host::set_device_event_store`]. Containers + items cross as JSON in
/// the `cal_core` wire shape — the native side maps `EKEvent`/`EKReminder` →
/// `Event`/`Task`, so the Rust adapter only parses. Mirrors [`KeychainBridge`];
/// the boundary is synchronous (the native side handles EventKit's async
/// internally before returning).
#[uniffi::export(with_foreign)]
pub trait DeviceEventStoreBridge: Send + Sync {
    /// Run the OS permission prompt for the selected entity types; `true` iff
    /// access was granted. Drives the add-account "grant access" step.
    fn request_access(&self, events: bool, reminders: bool) -> Result<bool, DeviceCalError>;
    /// Whether this platform exposes a reminders/tasks store (iOS yes, Android
    /// no) — gates the Tasks capability on the device adapter.
    fn supports_reminders(&self) -> bool;
    /// JSON `Vec<Calendar>`.
    fn list_calendars(&self) -> Result<String, DeviceCalError>;
    /// JSON `Vec<Event>` for `calendar_id` within `[start, end]` (RFC 3339).
    fn get_events(
        &self,
        calendar_id: String,
        start: String,
        end: String,
    ) -> Result<String, DeviceCalError>;
    /// `event_json` is a `NewEvent`; returns the created `Event` JSON.
    fn create_event(
        &self,
        calendar_id: String,
        event_json: String,
    ) -> Result<String, DeviceCalError>;
    /// `event_json` is an `Event`; returns the updated `Event` JSON.
    fn update_event(&self, event_json: String) -> Result<String, DeviceCalError>;
    fn delete_event(&self, event_id: String) -> Result<(), DeviceCalError>;
    /// JSON `Vec<TaskList>` (the device's reminder lists).
    fn list_reminder_lists(&self) -> Result<String, DeviceCalError>;
    /// JSON `Vec<Task>` for one reminder list.
    fn get_reminders(&self, list_id: String) -> Result<String, DeviceCalError>;
    /// `task_json` is a `NewTask`; returns the created `Task` JSON.
    fn create_reminder(&self, list_id: String, task_json: String)
        -> Result<String, DeviceCalError>;
    /// `task_json` is a `Task`; returns the updated `Task` JSON.
    fn update_reminder(&self, task_json: String) -> Result<String, DeviceCalError>;
    fn delete_reminder(&self, task_id: String) -> Result<(), DeviceCalError>;
}

/// Adapts a foreign [`DeviceEventStoreBridge`] to the engine-side
/// [`DeviceCalendarProvider`] the device adapter delegates to. Mirrors
/// [`BridgeSecretStore`].
struct BridgeDeviceProvider {
    bridge: Arc<dyn DeviceEventStoreBridge>,
}

fn to_core_dev_err(e: DeviceCalError) -> cal_core::Error {
    match e {
        DeviceCalError::PermissionDenied => {
            cal_core::Error::Unsupported("device calendar permission denied".into())
        }
        DeviceCalError::Unavailable => {
            cal_core::Error::Unsupported("device calendar/reminders unavailable".into())
        }
        DeviceCalError::Backend { detail } => cal_core::Error::internal(detail),
    }
}

impl DeviceCalendarProvider for BridgeDeviceProvider {
    fn request_access(&self, events: bool, reminders: bool) -> cal_core::Result<bool> {
        self.bridge
            .request_access(events, reminders)
            .map_err(to_core_dev_err)
    }

    fn supports_reminders(&self) -> bool {
        self.bridge.supports_reminders()
    }

    fn list_calendars(&self) -> cal_core::Result<String> {
        self.bridge.list_calendars().map_err(to_core_dev_err)
    }

    fn get_events(&self, calendar_id: &str, start: &str, end: &str) -> cal_core::Result<String> {
        self.bridge
            .get_events(calendar_id.to_string(), start.to_string(), end.to_string())
            .map_err(to_core_dev_err)
    }

    fn create_event(&self, calendar_id: &str, event_json: &str) -> cal_core::Result<String> {
        self.bridge
            .create_event(calendar_id.to_string(), event_json.to_string())
            .map_err(to_core_dev_err)
    }

    fn update_event(&self, event_json: &str) -> cal_core::Result<String> {
        self.bridge
            .update_event(event_json.to_string())
            .map_err(to_core_dev_err)
    }

    fn delete_event(&self, event_id: &str) -> cal_core::Result<()> {
        self.bridge
            .delete_event(event_id.to_string())
            .map_err(to_core_dev_err)
    }

    fn list_reminder_lists(&self) -> cal_core::Result<String> {
        self.bridge.list_reminder_lists().map_err(to_core_dev_err)
    }

    fn get_reminders(&self, list_id: &str) -> cal_core::Result<String> {
        self.bridge
            .get_reminders(list_id.to_string())
            .map_err(to_core_dev_err)
    }

    fn create_reminder(&self, list_id: &str, task_json: &str) -> cal_core::Result<String> {
        self.bridge
            .create_reminder(list_id.to_string(), task_json.to_string())
            .map_err(to_core_dev_err)
    }

    fn update_reminder(&self, task_json: &str) -> cal_core::Result<String> {
        self.bridge
            .update_reminder(task_json.to_string())
            .map_err(to_core_dev_err)
    }

    fn delete_reminder(&self, task_id: &str) -> cal_core::Result<()> {
        self.bridge
            .delete_reminder(task_id.to_string())
            .map_err(to_core_dev_err)
    }
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

    /// The cache observer as the `dyn CacheObserver` the SWR spawns + the
    /// refresher take (return-position unsizing coercion from the concrete
    /// `Arc<BridgeCacheObserver>`).
    fn observer(&self) -> Arc<dyn CacheObserver> {
        self.cache_observer.clone()
    }

    /// Invalidate the events snapshot for `calendar_id` after a mutation, so the
    /// next read sees it as cold and re-fetches (the cold-fallback serves a live
    /// read → the edit shows immediately). A no-op for the local account (there's
    /// no cache row). Mirrors the desktop `cache.invalidate` after external event
    /// writes.
    fn invalidate_events_cache(&self, calendar_id: &str) {
        let account = self
            .registry
            .account_for_calendar(calendar_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        let _ = self
            .cache
            .invalidate(&account, SyncScope::Events, calendar_id);
    }

    /// Invalidate the tasks snapshot for `list_id` after a task mutation, so the
    /// next read sees it as cold and re-fetches (the cold-fallback serves a live
    /// read → the edit shows immediately). A no-op for the local account (there's
    /// no cache row). Mirrors `invalidate_events_cache`.
    fn invalidate_tasks_cache(&self, list_id: &str) {
        let account = self
            .registry
            .account_for_task_list(list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        let _ = self.cache.invalidate(&account, SyncScope::Tasks, list_id);
    }

    /// Write-through after a successful EXTERNAL task mutation — the shared
    /// `CacheStore::write_through_task` (upsert the returned row into the
    /// retained snapshot, then mark stale) with this host's account
    /// resolution. Best-effort: a failed write-through costs staleness,
    /// never correctness.
    fn write_through_task(&self, task: &cal_core::Task) {
        let account = self
            .registry
            .account_for_task_list(&task.list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        let _ = self.cache.write_through_task(&account, &task.list_id, task);
    }

    /// Delete-side twin of `write_through_task`: drop the row from the
    /// retained snapshot so the next read doesn't resurrect it.
    fn write_through_task_removal(&self, list_id: &str, task_id: &str) {
        let account = self
            .registry
            .account_for_task_list(list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        let _ = self
            .cache
            .write_through_task_removal(&account, list_id, task_id);
    }

    /// Invalidate the SECTIONS snapshot for `list_id` after a section mutation
    /// (create / rename / delete / recolor), so the next `sections_json` sees it
    /// as cold and re-fetches. A no-op for the local account (no cache row).
    /// Mirrors `invalidate_tasks_cache` — sections share the list's account.
    fn invalidate_sections_cache(&self, list_id: &str) {
        let account = self
            .registry
            .account_for_task_list(list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        let _ = self
            .cache
            .invalidate(&account, SyncScope::Sections, list_id);
    }

    /// Invalidate the contacts snapshot for `list_id` after a contact mutation,
    /// so the next read sees it as cold and re-fetches. A no-op for the local
    /// account (no cache row). Mirrors `invalidate_events_cache`.
    fn invalidate_contacts_cache(&self, list_id: &str) {
        let account = self
            .registry
            .account_for_contact_list(list_id)
            .unwrap_or_else(|| LOCAL_ID.to_string());
        let _ = self
            .cache
            .invalidate(&account, SyncScope::Contacts, list_id);
    }

    /// Invalidate an account's CALENDAR-listing snapshot after an external
    /// calendar's listing metadata changes (a provider-side rename), so the next
    /// `list_calendars_json` re-fetches the catalogue and shows the new name.
    /// The listing scope has no per-container id (it's the account's whole
    /// catalogue), so the container key is `""` — same key the listing SWR
    /// reads/writes. A no-op for the local account (no cache row).
    fn invalidate_calendars_listing(&self, account: &str) {
        let _ = self.cache.invalidate(account, SyncScope::Calendars, "");
    }

    /// Invalidate an account's TASK-LIST-listing snapshot after an external task
    /// list's listing metadata changes (a provider-side rename). Mirrors
    /// `invalidate_calendars_listing`.
    fn invalidate_task_lists_listing(&self, account: &str) {
        let _ = self.cache.invalidate(account, SyncScope::TaskLists, "");
    }

    /// Invalidate an account's CONTACT-LIST-listing snapshot after an external
    /// address book's listing metadata changes (a provider-side rename). Mirrors
    /// `invalidate_calendars_listing`.
    fn invalidate_contact_lists_listing(&self, account: &str) {
        let _ = self.cache.invalidate(account, SyncScope::ContactLists, "");
    }

    /// Append one `sync_log` row (best-effort: a logging failure must never sink
    /// a sync round, and cal-ffi has no logger, so the error is swallowed).
    fn record_sync_round(
        &self,
        trigger: SyncTrigger,
        success: bool,
        counters: &SyncLogCounters,
        duration_ms: Option<u64>,
        error: Option<&str>,
    ) {
        let shared = self.db.shared();
        let repo = SyncLogRepo::new(&shared);
        let _ = repo.record(trigger, success, counters, duration_ms, error);
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
        // Refuse a dataset that needs a newer Aperio than this build, before
        // anything is written. The desktop has always checked here; this host
        // only checked on the JOIN path (`preview_sync_target`, which the form
        // reads as `app_too_old`), so SWAPPING to a too-new target was accepted
        // and only surfaced later, at round time, where nobody is looking.
        //
        // It goes ahead of the E2E branch deliberately: that branch WRITES
        // `PREF_E2E_ENABLED` as a side effect, and refusing after it would leave
        // the flag set for a target this device just declined.
        //
        // The same version string the orchestrator was built with (host.rs's
        // `build_orchestrator` call), so the two cannot disagree about what is
        // running.
        if let Some(m) = meta.as_ref() {
            sync_core::ensure_compatible(m, env!("CARGO_PKG_VERSION")).map_err(sync_err)?;
        }
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
        // "The stored keychain secret" is no longer one fixed pseudo-account per
        // kind. Once this device syncs through an account row the credential
        // lives under that row's id, so the lookup goes through
        // `sync_target::stored_secret`, which knows both places and which of
        // them is the newer answer.
        let shared = self.db.shared();
        let held = |kind: &str, slot: SecretSlot| {
            host_core::sync_target::stored_secret(
                &UserPrefsRepo::new(&shared),
                &AccountsRepo::new(&shared),
                self.secret_store.as_ref(),
                kind,
                slot,
            )
        };
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
                    _ => held("webdav", SecretSlot::Password),
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
                    _ => held("ftp", SecretSlot::Password).ok_or_else(|| StoreError::Auth {
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
                let refresh_token =
                    held("dropbox", SecretSlot::RefreshToken).ok_or_else(|| StoreError::Auth {
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
                let refresh_token =
                    held("googledrive", SecretSlot::RefreshToken).ok_or_else(|| {
                        StoreError::Auth {
                            detail: "Google Drive sign-in required — no refresh token stored"
                                .to_string(),
                        }
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
                                _ => held("sftp", SecretSlot::Password).ok_or_else(|| {
                                    StoreError::Auth {
                                        detail: "no SFTP password configured".to_string(),
                                    }
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
                                // Its own slot, kept apart from the password so
                                // switching method never clobbers the other.
                                _ => held("sftp", SecretSlot::KeyPassphrase).unwrap_or_default(),
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
                    "auth_method": if auth_method == "key" { "key" } else { "password" },
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

    /// Run the registry's ephemeral credential probe (block_on the async
    /// fan-out) and map the outcome: a non-credential kind (Local / OAuth) is a
    /// silent skip, a missing/invalid field is an InvalidField, anything else
    /// surfaces its message. Shared by `test_account_json` + the create-time
    /// smoke-test.
    fn probe_account(
        &self,
        adapter_kind: &AdapterKind,
        config_json: &str,
        secret: Option<&str>,
    ) -> Result<(), StoreError> {
        use host_core::registry::RegistryError as Re;
        match self.runtime.block_on(
            self.registry
                .probe_account(adapter_kind, config_json, secret),
        ) {
            Ok(()) | Err(Re::Unsupported(_)) => Ok(()),
            Err(Re::Secret(detail)) => Err(StoreError::InvalidField {
                field: "secret".to_string(),
                detail,
            }),
            Err(Re::Config(detail)) => Err(StoreError::InvalidField {
                field: "config".to_string(),
                detail,
            }),
            Err(other) => Err(StoreError::Storage {
                detail: other.to_string(),
            }),
        }
    }

    /// Write the chosen target down as the account row this device syncs
    /// through, via `host_core::sync_target`, which owns the per-kind knowledge
    /// and the tests that cover it — and which retires the `sync.adapter.*`
    /// preferences and keychain pseudo-accounts this used to write, so the two
    /// records cannot disagree.
    ///
    /// All this does is flatten the typed request into the value map that module
    /// takes. There is no branching on kind here, on purpose: this host and the
    /// desktop each used to carry their own six-arm match, and the two had
    /// drifted — one storing credentials for an authentication method the user
    /// had not chosen, the other writing a sentinel most readers never
    /// recognised.
    ///
    /// Only a non-empty value is put in the map, which is what keeps the reuse
    /// contract: an omitted secret is inherited from what this device already
    /// holds rather than cleared.
    fn persist_sync_config(&self, req: &ConfigureSyncRequest) -> Result<(), StoreError> {
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        let mut values = serde_json::Map::new();
        let mut put = |key: &str, value: Option<&str>| {
            if let Some(v) = value {
                values.insert(key.to_string(), serde_json::Value::String(v.to_string()));
            }
        };
        put("path", req.path.as_deref());
        put("url", req.url.as_deref());
        put("user", req.user.as_deref());
        put("password", req.password.as_deref());
        put("host", req.host.as_deref());
        put("mode", req.mode.as_deref());
        put("client_id", req.client_id.as_deref());
        put("client_secret", req.client_secret.as_deref());
        put("folder_name", req.folder_name.as_deref());
        put("auth_method", req.auth_method.as_deref());
        put("key_path", req.key_path.as_deref());
        put("key_passphrase", req.key_passphrase.as_deref());
        if let Some(port) = req.port {
            values.insert("port".to_string(), serde_json::Value::from(port));
        }

        host_core::sync_target::connect(
            &prefs,
            &AccountsRepo::new(&shared),
            self.secret_store.as_ref(),
            &HostSyncPlugins(&self.plugin_manager),
            &req.kind,
            &values,
        )
        .map(|_| ())
        .map_err(storage_err)
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

    /// Resolve the EXTERNAL task adapter owning `list_id`, or `Unsupported` when
    /// the list is local / has no provider. Member-management writes (§9.7) only
    /// make sense against a sharing-capable provider — the UI gates them on the
    /// `manageable` capability, which the local store never advertises, so this
    /// is a belt-and-braces guard rather than a reachable error. Mirrors the
    /// desktop `route_task_list` helper (which 404s a non-routable list).
    fn route_task_list_external(&self, list_id: &str) -> Result<Arc<dyn TasksFeature>, StoreError> {
        self.route_task_list(list_id)?
            .ok_or_else(|| StoreError::Unsupported {
                detail: "member management is only available on external task lists".to_string(),
            })
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
        // Which plugin serves this kind is the PLUGIN's own statement, read
        // from its manifest — the host keeps no kind→plugin table.
        account_kinds
            .get(account_id)
            .and_then(|kind| self.plugin_manager.plugin_for_adapter_kind(kind.as_str()))
            .map(|p| p.manifest.tasks.clone())
            .unwrap_or_default()
    }

    /// Resolve an account's recurrence capabilities from its plugin manifest
    /// (mirrors the desktop `recurrence_caps_for_account`): the local store + any
    /// account whose plugin we can't resolve fall back to full RFC-5545 support —
    /// the host's own SQLite store has no restrictions, and a missing manifest
    /// shouldn't silently strip options the source might actually support.
    fn recurrence_caps_for_account(
        &self,
        account_id: &str,
        account_kinds: &std::collections::HashMap<String, AdapterKind>,
    ) -> RecurrenceCapabilities {
        if account_id == LOCAL_ID {
            return RecurrenceCapabilities::default();
        }
        let Some(plugin) = account_kinds
            .get(account_id)
            .and_then(|kind| self.plugin_manager.plugin_for_adapter_kind(kind.as_str()))
        else {
            return RecurrenceCapabilities::default();
        };
        Some(plugin)
            .map(|p| p.manifest.recurrence.clone())
            .unwrap_or_default()
    }

    /// Apply the `TakeRemote` conflict resolution: write the stored
    /// `remote_value` into the local row + emit the matching `*Updated`
    /// SyncEvent so other devices converge. Mirrors the desktop
    /// `apply_take_remote` (against `self.adapter` + `self.writer`).
    fn apply_take_remote(&self, record: &ConflictRecord) -> Result<(), StoreError> {
        // `remote_value` is JSON-encoded — Null when the remote cleared the field.
        let remote_value: serde_json::Value = match &record.remote_value {
            Some(raw) => serde_json::from_str(raw).map_err(|err| StoreError::Storage {
                detail: format!("decode remote value: {err}"),
            })?,
            None => serde_json::Value::Null,
        };
        match record.row_kind {
            ConflictKind::Event => {
                let mut row = self
                    .adapter
                    .get_event_by_id(&record.row_id)
                    .map_err(map_store_err)?
                    .ok_or(StoreError::NotFound)?;
                patch_field(&mut row, &record.field, &remote_value)?;
                row.updated_at = chrono::Utc::now();
                self.adapter
                    .upsert_event_from_sync(&row)
                    .map_err(map_store_err)?;
                self.writer.append(SyncEvent::EventUpdated(EventPayload {
                    id: row.id.clone(),
                    fields: serde_json::to_value(&row).unwrap_or_default(),
                }));
            }
            ConflictKind::Task => {
                let mut row = self
                    .adapter
                    .get_task_by_id(&record.row_id)
                    .map_err(map_store_err)?
                    .ok_or(StoreError::NotFound)?;
                patch_field(&mut row, &record.field, &remote_value)?;
                row.updated_at = chrono::Utc::now();
                self.adapter
                    .upsert_task_from_sync(&row)
                    .map_err(map_store_err)?;
                self.writer.append(SyncEvent::TaskUpdated(EventPayload {
                    id: row.id.clone(),
                    fields: serde_json::to_value(&row).unwrap_or_default(),
                }));
            }
            ConflictKind::TaskList => {
                let mut row = self
                    .adapter
                    .get_task_list_by_id(&record.row_id)
                    .map_err(map_store_err)?
                    .ok_or(StoreError::NotFound)?;
                patch_field(&mut row, &record.field, &remote_value)?;
                self.adapter
                    .upsert_task_list_from_sync(&row)
                    .map_err(map_store_err)?;
                self.writer.append(SyncEvent::TaskListUpdated(EventPayload {
                    id: row.id.clone(),
                    fields: serde_json::to_value(&row).unwrap_or_default(),
                }));
            }
            ConflictKind::Calendar => {
                let mut row = self
                    .adapter
                    .get_calendar_by_id(&record.row_id)
                    .map_err(map_store_err)?
                    .ok_or(StoreError::NotFound)?;
                patch_field(&mut row, &record.field, &remote_value)?;
                self.adapter
                    .upsert_calendar_from_sync(&row)
                    .map_err(map_store_err)?;
                self.writer.append(SyncEvent::CalendarUpdated(EventPayload {
                    id: row.id.clone(),
                    fields: serde_json::to_value(&row).unwrap_or_default(),
                }));
            }
            ConflictKind::ColorLabel => {
                let mut row = self
                    .adapter
                    .get_color_label_by_id(&record.row_id)
                    .map_err(map_store_err)?
                    .ok_or(StoreError::NotFound)?;
                patch_field(&mut row, &record.field, &remote_value)?;
                self.adapter
                    .upsert_color_label_from_sync(&row)
                    .map_err(map_store_err)?;
                self.writer
                    .append(SyncEvent::ColorLabelUpdated(EventPayload {
                        id: row.id.0.clone(),
                        fields: serde_json::to_value(&row).unwrap_or_default(),
                    }));
            }
        }
        Ok(())
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
    recurrence_capabilities: RecurrenceCapabilities,
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
        // Its own column, so the planned block round-trips exactly.
        task_span: true,
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
    // The built-in store's sync half is linked in, not loaded. Every path that
    // opens a sync adapter comes through here — the schema path, the legacy
    // preferences, the onboarding preview — so this is the one gate it needs.
    if let Some(built) = host_core::builtin_adapters::open_sync(plugin_id, &config_json) {
        return built.map_err(|detail| StoreError::InvalidField {
            field: "remote_root".to_string(),
            detail,
        });
    }
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

/// How this host answers `host_core`'s two questions about a sync plugin: which
/// one serves a kind and what its account schema says, and how to open an
/// instance of it.
///
/// A free struct rather than a method, because the restore below runs in
/// [`Host::open`] before `Self` exists.
struct HostSyncPlugins<'a>(&'a PluginManager);

impl host_core::sync_target::SyncPlugins for HostSyncPlugins<'_> {
    fn resolve(
        &self,
        adapter_kind: &str,
    ) -> Option<(String, plugin_core::account_schema::AccountSchema)> {
        // The built-in store first. It declares that it can hold the dataset
        // and has no vtable to do it with, so its execution is a plugin's —
        // see `host_core::builtin_adapters::sync_plugin_for`. Asked first
        // because no plugin declares its kind, so the lookup below would
        // simply miss and the built-in account could never be a sync target.
        if let Some(found) = host_core::builtin_adapters::sync_plugin_for(adapter_kind) {
            return Some(found);
        }
        let plugin = self.0.plugin_for_adapter_kind(adapter_kind)?;
        let schema = plugin.manifest.account.clone()?;
        Some((plugin.manifest.id.clone(), schema))
    }

    fn open(&self, plugin_id: &str, config_json: String) -> Result<Arc<dyn SyncAdapter>, String> {
        open_sync_plugin(self.0, plugin_id, config_json).map_err(|err| err.to_string())
    }
}

/// Restore what this device syncs through, for [`Host::open`] on app start —
/// the mobile twin of the desktop `restore_sync_adapter`.
///
/// The migration, the choice of reader and the log line are
/// `host_core::sync_target`'s, shared with the desktop and tested there; this
/// host contributes the four arguments only it can produce — its database, its
/// keychain bridge, its host-key pin store, and [`HostSyncPlugins`]. Runs before
/// `Self` exists, so it takes the plugin manager and the secret store by
/// reference.
///
/// The returned adapter is already wrapped for encryption where this device
/// encrypts, so the caller configures it exactly as it stands.
fn restore_sync_adapter(
    shared: &SharedConn,
    prefs: &UserPrefsRepo,
    plugin_manager: &PluginManager,
    secret_store: &dyn SecretStore,
) -> Option<Arc<dyn SyncAdapter>> {
    host_core::sync_target::restore_sync_target(
        prefs,
        &AccountsRepo::new(shared),
        secret_store,
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &HostSyncPlugins(plugin_manager),
    )
}

/// Reconstruct the sync adapter this device is configured to open, WITHOUT the
/// migration and without the start-up log line — the mobile twin of the desktop
/// `build_adapter_from_prefs`. Best-effort: a missing/blank field or an open
/// failure yields `None`, leaving sync unconfigured until the user
/// re-configures from the Sync screen.
///
/// Which of the two records answers is `build_for_device`'s decision: the
/// account row when this device points at one, and only then the
/// `sync.adapter.*` preferences it has not moved off yet.
///
/// Start-up does not come through here any more — it calls
/// [`restore_sync_adapter`]. What is left are the two encryption paths, which
/// need a second handle to the target the orchestrator already holds, at a
/// moment where the migration has long since run and a second "restored the
/// sync target" line would be a lie about what just happened.
fn restore_adapter_from_prefs(
    shared: &SharedConn,
    prefs: &UserPrefsRepo,
    plugin_manager: &PluginManager,
    secret_store: &dyn SecretStore,
) -> Option<Arc<dyn SyncAdapter>> {
    match host_core::sync_target::build_for_device(
        prefs,
        &AccountsRepo::new(shared),
        secret_store,
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &HostSyncPlugins(plugin_manager),
    ) {
        Ok(adapter) => Some(adapter),
        Err(host_core::sync_target::Unbuildable::NotConfigured) => None,
        Err(err) => {
            tracing::warn!(%err, "could not restore the configured sync target");
            None
        }
    }
}

/// The two refusals a grouping request can meet are answers, not failures.
///
/// `TooFewMembers` cannot reach a user through the UI (the call sites always
/// name two events), so it maps to invalid input. `ConflictingGroups` can and
/// does: it means both events are already grouped, with different partners,
/// and only the user can decide what that should become. `Conflict` is the
/// only thing the grouping call raises, so the call site can phrase it
/// exactly.
fn map_group_err(err: EventGroupsError) -> StoreError {
    match err {
        EventGroupsError::TooFewMembers => StoreError::InvalidField {
            field: "members".to_string(),
            detail: err.to_string(),
        },
        EventGroupsError::ConflictingGroups => StoreError::Conflict {
            detail: err.to_string(),
        },
        EventGroupsError::Sqlite(err) => StoreError::Storage {
            detail: err.to_string(),
        },
        EventGroupsError::Vanished => StoreError::Storage {
            detail: err.to_string(),
        },
    }
}

/// Device-calendar helpers that take the host-internal
/// [`DeviceCalendarProvider`] (not a UniFFI type), so they live OUTSIDE the
/// `#[uniffi::export]` block — UniFFI must not try to expose them.
impl Host {
    /// Snapshot the installed device provider, if any.
    fn device_provider(&self) -> Option<Arc<dyn DeviceCalendarProvider>> {
        self.device_provider.read().ok().and_then(|g| g.clone())
    }

    /// Build the device adapter over `provider` and insert it into the registry
    /// under `account_id` (Calendar always; Tasks only where the provider has a
    /// reminders store, i.e. iOS).
    fn register_device_adapter(&self, account_id: &str, provider: Arc<dyn DeviceCalendarProvider>) {
        let supports_reminders = provider.supports_reminders();
        let adapter = Arc::new(DeviceAdapter::new(provider));
        let cal: Arc<dyn cal_core::CalendarFeature> = adapter.clone();
        let tasks: Option<Arc<dyn cal_core::TasksFeature>> =
            supports_reminders.then_some(adapter as Arc<dyn cal_core::TasksFeature>);
        self.registry
            .register_host_adapter(account_id, Some(cal), tasks);
    }

    /// Bring up adapters for external accounts that arrived through SYNC.
    ///
    /// Accounts created on another device — or restored into a fresh install
    /// by the onboarding wizard — are written into the `accounts` table by
    /// the event-log applier; they never pass through the add-account paths
    /// that register an adapter. Without this the account row exists (its
    /// name shows up) but it has no adapter, so it lists no calendars / task
    /// lists / address books and shows no items until the next app start
    /// re-runs `bootstrap`. The desktop twin lives in the sync scheduler's
    /// post-round hook (`SyncScheduler::register_synced_accounts`).
    ///
    /// Only ADAPTER-LESS accounts are registered: rebuilding a live
    /// account's adapter would throw away the plugin instance's in-memory
    /// provider state and force a cold re-drain. Accounts whose credentials
    /// aren't on this device fail to register — expected (§19.11's reconnect
    /// wizard covers them); `register_missing` logs and moves on.
    ///
    /// Nothing new ⇒ nothing else happens: this runs after every applying
    /// round, so an unconditional warm pass would be constant background
    /// noise.
    fn register_synced_accounts(&self) {
        let registered = {
            let shared = self.db.shared();
            let repo = AccountsRepo::new(&shared);
            self.registry.register_missing(&repo)
        };
        if registered == 0 {
            return;
        }
        tracing::info!(
            target: "aperio::registry",
            registered,
            "registered adapters for accounts that arrived through sync",
        );
        // Fresh adapters have nothing cached, so warm them: the pass
        // enumerates the new accounts' containers and fills their items.
        // AUTOMATIC, so it runs UN-forced (like warm_cache_on_foreground) —
        // a cold-start network blip must be confirmed by a second attempt
        // before it surfaces as a per-account error.
        let refresher = Arc::clone(&self.cache_refresher);
        self.runtime.handle().spawn(async move {
            refresher.warm_all(false).await;
        });
        // …and tell the UI to re-read its catalogs NOW instead of waiting out
        // the pass. The listing scopes are the channel the app already uses
        // for "the container lists changed" (cacheObserver.ts maps them to
        // the calendar / tasks / contacts reload categories). The JS side
        // keys off `scope` alone; the ids stay empty because this signal is
        // account-wide, not per-container.
        for scope in [
            SyncScope::Calendars,
            SyncScope::TaskLists,
            SyncScope::ContactLists,
        ] {
            self.cache_observer.cache_updated(&CacheUpdatedPayload {
                scope: scope.as_str().to_string(),
                account_id: String::new(),
                container_id: String::new(),
            });
        }
    }
}

/// A `YYYY-MM-DD` day key from the foreign side.
///
/// A free function rather than an associated one: the `#[uniffi::export]` impl
/// block cannot hold associated functions, and UniFFI would have tried to
/// expose it.
///
/// The frontends speak local day keys everywhere (`shared/dateKey.ts`), so
/// this is the one place the string becomes a date. A malformed one is the
/// caller's bug, reported as such rather than silently becoming today.
fn parse_day_key(day: &str) -> Result<chrono::NaiveDate, StoreError> {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| StoreError::InvalidField {
        field: "day".to_string(),
        detail: format!("not a YYYY-MM-DD day: {day}"),
    })
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
        let adapter = LocalAdapter::new(db.shared()).with_read_pool(Arc::new(db.clone()));

        // Multi-thread runtime: `block_on` drives the CalendarFeature methods +
        // sync rounds, while the event-log writer's drain task + the SWR warm
        // pass's spawned per-container refreshes live on the worker threads. A
        // SMALL POOL (not one worker) so a warm pass refreshes several external
        // containers' HTTP reads concurrently instead of one-at-a-time — the
        // cache warms much faster after a sync, so the next day/week open serves
        // from cache. All these futures are already `Send` (multi_thread requires
        // it) + shared state is Arc/Mutex-guarded, so more workers is safe.
        // `enable_all` gives the time + I/O drivers the HTTP shim + writer need.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
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
        // Bring up the rolling-file log sink under <data_dir>/logs BEFORE the
        // heavy registry / orchestrator work so early traces are captured, then
        // apply the persisted level. Process-global + Once-guarded, so the many
        // Host::open calls in the test binary install it exactly once.
        crate::logging::init_mobile_logging(&data_dir);
        if let Ok(Some(level)) =
            UserPrefsRepo::new(&db.shared()).get(host_core::logging::PREF_LOG_LEVEL)
        {
            crate::logging::set_level(&level);
        }
        // The content-addressed custom-sound store, same convention + location as
        // the sync round (which already push/fetches it via build_orchestrator's
        // DesktopSyncRoundHooks) and the desktop SoundsDir.
        let sounds_dir = host_core::sound_assets::sounds_dir_under(&data_dir);
        // Install the plugin→host channel BEFORE any instance is opened — see
        // the desktop side for why the ordering matters.
        host_core::plugin_channel::HostChannel::install(Arc::clone(&secret_store));
        let registry = Arc::new(AdapterRegistry::with_data_dir(
            Arc::clone(&plugin_manager),
            Arc::clone(&secret_store),
            Some(data_dir.clone()),
        ));
        // Before bootstrap, so the first registration already sees this
        // device's half of every account. Registering first and wiring after
        // would open every adapter once with the travelling half alone.
        registry.set_device_local_store(Arc::new(host_core::account_local::PrefsDeviceLocal::new(
            db.shared(),
        )));
        // Same moment, same reason. An adapter that pins host keys cannot be
        // opened at all until this is set, which is the safe direction to fail
        // in: refusing to connect is recoverable, connecting unverified is not.
        registry.set_host_key_pins(Arc::new(
            host_core::sftp_host_keys::UserPrefsHostKeyVerifier::new(db.shared()),
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
                Arc::clone(&plugin_manager),
                env!("CARGO_PKG_VERSION"),
                boot_at,
            )
        });

        // Restore a previously-configured sync target so `sync_now` works
        // without a re-configure step (the desktop's restore_sync_adapter, and
        // the same shared decision underneath): the one-way migration off this
        // device's old `sync.adapter.*` preferences runs first, then the account
        // row this device points at is read, and a device the migration has not
        // reached still comes up on its preferences. Which record answered, and
        // through which account id, is logged there.
        //
        // Best-effort: a missing/unbuildable target just leaves sync
        // unconfigured (the user re-configures from the Sync screen).
        //
        // After `registry.bootstrap` above, deliberately — the same ordering as
        // the desktop, for the same two reasons: a sync target holds no
        // calendars, no task lists and no contacts, so the registry skips the
        // row the migration creates (`is_sync_only`) and an earlier run would
        // buy nothing; and the account path asks the loaded manifests which
        // plugin serves the kind, so it cannot run before registration.
        //
        // §19.7's encryption wrap is NOT applied here any more. It lives in the
        // shared reader, which is the only place that can decide it once —
        // wrapping again on top of an already-wrapped adapter encrypted every
        // upload twice, which no other device could read. The refusal that used
        // to live here is still there, in that same reader: the flag set with
        // the key gone (keychain wiped / fresh OS install on the same data dir)
        // refuses rather than pushing plaintext to an encrypted dataset.
        {
            let shared = db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            if let Some(adapter) =
                restore_sync_adapter(&shared, &prefs, &plugin_manager, secret_store.as_ref())
            {
                graph.orchestrator.configure(adapter);
            }
        }

        // Credentials that predate their own syncability — the desktop twin
        // sits beside its other one-shot backfills in lib.rs. E2E-gated and
        // versioned inside; a no-op everywhere but the first launch after an
        // upgrade that widened the syncable slots.
        host_core::credential_sync::backfill_new_syncable_slots(
            &graph.writer,
            &db.shared(),
            &plugin_manager,
            secret_store.as_ref(),
        );
        // Settings that joined the sync whitelist after they were written
        // (the signature list) — the desktop's twin sits beside its
        // credential backfill in lib.rs. Versioned inside.
        host_core::settings_backfill::backfill_newly_synced_settings(&graph.writer, &db.shared());

        // The external-adapter snapshot cache. Construct it (+ the refresh dedup
        // coordinator + the swappable observer + the refresher), mirroring the
        // desktop lib.rs. The periodic warm loop is NOT started — mobile drives
        // warm passes manually + on foreground (refresh_external_cache /
        // warm_cache_on_foreground). The observer is a no-op until the JS layer
        // registers its bridge; background refreshes populate the cache anyway.
        let cache = Arc::new(CacheStore::new(db.clone()));
        let coord = Arc::new(RefreshCoordinator::new());

        // One-time auto re-bootstrap on a cache-generation bump (same as the
        // desktop lib.rs): on upgrade to a version whose event mapping changed
        // (e.g. the recurrence-timezone fix), already-cached payloads are stale
        // and a delta sync won't re-fetch unchanged events. Clear every external
        // account's sync state ONCE so the next warm pass re-bootstraps + re-maps.
        // Idempotent (a `user_prefs` marker); best-effort — logs and continues.
        {
            let shared = db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            match AccountsRepo::new(&shared).list() {
                Ok(accts) => {
                    match host_core::cache::reconcile_cache_generation(&cache, &accts, &prefs) {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(
                            target: "aperio::cache",
                            containers = n,
                            generation = host_core::cache::CACHE_GENERATION,
                            "cache generation upgrade: cleared external sync state for a one-time re-bootstrap",
                        ),
                        Err(err) => tracing::warn!(
                            %err,
                            "cache generation reconcile failed; will retry next boot",
                        ),
                    }
                }
                Err(err) => {
                    tracing::warn!(?err, "cache generation reconcile: couldn't list accounts")
                }
            }
        }

        let cache_observer = Arc::new(BridgeCacheObserver::new());
        let cache_refresher = CacheRefresher::new(
            Arc::clone(&registry),
            Arc::clone(&cache),
            Arc::clone(&coord),
            db.shared(),
            Arc::clone(&cache_observer) as Arc<dyn CacheObserver>,
        );

        // The contact-sync core, sharing the bootstrapped registry. No worker
        // loop is started — mobile drives `run_sync` from the manual button /
        // foreground triggers (the desktop's tokio loop has no mobile twin). The
        // observer is a no-op until the JS layer registers its bridge.
        let contact_sync_observer = Arc::new(BridgeContactSyncObserver::new());
        let contact_sync = ContactSyncCore::new(Arc::clone(&registry), db.shared());

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
            cache,
            coord,
            cache_observer,
            cache_refresher,
            contact_sync,
            contact_sync_observer,
            device_provider: std::sync::RwLock::new(None),
            sounds_dir,
        }))
    }

    /// All persisted accounts as JSON (the `cal_core`/desktop wire shape),
    /// each with the derived flags the desktop listing also carries.
    ///
    /// `is_videoconference` comes from the plugin's declared TYPE rather than
    /// from a list of provider names, so an adapter added later is offered by
    /// the editor's "create meeting" control without a change here or in the
    /// UI. `plugin_loaded` mirrors the desktop's, so a missing plugin reads the
    /// same on both platforms.
    pub fn accounts_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let accounts = repo.list().map_err(acc_err)?;
        let enriched: Vec<serde_json::Value> = accounts
            .into_iter()
            .map(|account| {
                let plugin = self
                    .plugin_manager
                    .plugin_for_adapter_kind(account.adapter_kind.as_str());
                let is_videoconference = plugin.as_ref().is_some_and(|p| {
                    p.manifest
                        .has_capability(&plugin_core::Capability::Videoconference)
                });
                let plugin_loaded = account.adapter_kind.is_host_internal() || plugin.is_some();
                let mut value = serde_json::to_value(&account).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("plugin_loaded".into(), plugin_loaded.into());
                    obj.insert("is_videoconference".into(), is_videoconference.into());
                }
                value
            })
            .collect();
        to_json(&enriched)
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

        // This path builds a row from values the caller already has. Two kinds
        // of account cannot come through it, and neither is decided by name:
        // one whose adapter is not loaded (nothing would register), and one
        // whose adapter signs in through the provider (the row would persist
        // without the grant that makes it work). Everything else — including a
        // host-internal kind like the device calendar, whose "auth" is the OS
        // permission prompt — is fine.
        let schema = host_core::account_setup::schema_for_kind(
            &self.plugin_manager,
            req.adapter_kind.as_str(),
        );
        if !req.adapter_kind.is_host_internal() {
            let Some(schema) = schema.as_ref() else {
                return Err(StoreError::InvalidField {
                    field: "adapter_kind".to_string(),
                    detail: format!(
                        "no loaded adapter declares accounts of kind {}",
                        req.adapter_kind.as_str()
                    ),
                });
            };
            if host_core::account_setup::signs_in_with_oauth(schema) {
                return Err(StoreError::InvalidField {
                    field: "adapter_kind".to_string(),
                    detail: format!(
                        "adapter '{}' signs in through its provider; use the connect flow",
                        req.adapter_kind.as_str()
                    ),
                });
            }
        }

        // The device-calendar account is host-internal: it carries no remote
        // credential (access is the OS permission prompt, run via
        // `request_device_calendar_access` before this call) and needs the native
        // bridge to register. Bail early if the bridge isn't installed (e.g. the
        // kind was requested on a platform without one) so we never persist a row
        // that can't come up.
        let is_device = req.adapter_kind == host_core::builtin_adapters::device_calendar_kind();
        let device_provider = if is_device {
            Some(
                self.device_provider()
                    .ok_or_else(|| StoreError::InvalidField {
                        field: "adapter_kind".to_string(),
                        detail: "device calendar is not available on this platform".to_string(),
                    })?,
            )
        } else {
            None
        };

        // Pre-persist credential smoke-test (desktop parity): probe the entered
        // creds with an ephemeral adapter BEFORE writing any row, so a bad
        // password / unreachable host fails here instead of leaving a
        // saved-but-broken account. Local has no remote → probe_account skips it.
        // Gated out of unit-test builds: the probe does live network (an on-device
        // concern, like the test_account_json probe tests); the no-network
        // validation branches stay covered. Production always smoke-tests.
        // The device adapter has no remote endpoint to probe (access is the OS
        // permission grant, already obtained); every other non-local kind smoke-
        // tests its credentials here.
        #[cfg(not(test))]
        if !is_device {
            self.probe_account(&req.adapter_kind, &req.config_json, req.secret.as_deref())?;
        }

        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let created = repo
            .create(
                req.adapter_kind.clone(),
                req.display_name.trim(),
                &req.config_json,
            )
            .map_err(acc_err)?;

        // Persist the secret right after the row so the keychain and DB stay
        // aligned. Which slot is the adapter's own statement — the same one the
        // registry reads back at open time, so the two cannot disagree. A write
        // failure is fatal: tear the row down.
        if let Some(secret) = req.secret {
            let Some(slot) = schema
                .as_ref()
                .and_then(host_core::account_setup::repair_slot)
            else {
                let _ = repo.delete(&created.id);
                return Err(StoreError::InvalidField {
                    field: "secret".to_string(),
                    detail: format!(
                        "adapter kind {} declares no single credential field",
                        req.adapter_kind.as_str()
                    ),
                });
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
                &self.plugin_manager,
                &created.id,
                slot,
                &secret,
            );
        }

        // Register the freshly created adapter. A failure is fatal: drop the
        // secrets + row so keychain/DB/registry stay in step.
        if let Some(provider) = device_provider {
            // Host-internal device adapter — insert directly (no plugin, no
            // secret, can't fail). The provider's presence was checked above.
            self.register_device_adapter(&created.id, provider);
        } else if req.adapter_kind != "local" {
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
        // secret). Mirrors the desktop create_account. The device-calendar
        // exception (Option A) is no longer spelled out here: `device_calendar`
        // is host-internal, so the predicate already keeps it home, along with
        // every other account that belongs to the device that made it.
        append_account_event(
            &self.writer,
            &self.plugin_manager,
            created.adapter_kind.as_str(),
            SyncEvent::AccountCreated(account_payload(&created)),
        );

        to_json(&created)
    }

    /// Probe entered credentials WITHOUT persisting anything — open an ephemeral
    /// adapter from the (kind, config, secret) the add form assembles, run the
    /// kind's read probe, and drop it. `Ok(())` = the credentials work; a typed
    /// StoreError (its message names the cause) drives the UI banner. Reuses the
    /// NewAccountRequest wire shape (display_name is ignored). Mirrors the
    /// desktop `test_*_connection` commands.
    pub fn test_account_json(&self, request_json: String) -> Result<(), StoreError> {
        let req: NewAccountRequest = from_json("test account", &request_json)?;
        self.probe_account(&req.adapter_kind, &req.config_json, req.secret.as_deref())
    }

    /// Probe an adapter that declares a schema, from the form's own values.
    ///
    /// The twin of the desktop `test_account`, and the reason it exists rather
    /// than reusing [`Self::test_account_json`]: the values are split by the
    /// SAME `plan_new_account` the connect call uses, so a test and a connect
    /// cannot disagree about what a field means. The older entry point takes an
    /// already-split config, which puts that decision in the caller.
    pub fn test_account_values_json(&self, request_json: String) -> Result<(), StoreError> {
        let req: SchemaFormRequest = from_json("test account", &request_json)?;
        let (_, schema) = self.schema_for(&req.adapter_kind)?;
        // No OAuth client choice: a probe never signs in, so an adapter
        // reachable only with a token the sign-in produces has nothing to test
        // before the account exists, and says so by failing the probe.
        let values = match req.account_id.as_deref() {
            Some(account_id) => {
                host_core::account_update::inherit_stored_secrets(
                    self.secret_store.as_ref(),
                    account_id,
                    &schema,
                    &req.values,
                )
                .map_err(|err| StoreError::InvalidField {
                    field: "values".to_string(),
                    detail: err.to_string(),
                })?
                .0
            }
            None => req.values.clone(),
        };
        let plan =
            host_core::account_setup::plan_new_account(&schema, &values, None).map_err(|err| {
                StoreError::InvalidField {
                    field: "values".to_string(),
                    detail: err.to_string(),
                }
            })?;
        // At most one credential reaches a probe; a schema with several would
        // need the registry to take them all, which no adapter has asked for.
        let secret = plan.secrets.first().map(|(_, value)| value.as_str());
        self.probe_account(
            &AdapterKind::new(&req.adapter_kind),
            &plan.probe_config_json(),
            secret,
        )
    }

    /// Run one action an adapter declared on its connect form, and return the
    /// values the form should now carry, keyed by FIELD key.
    ///
    /// The twin of the desktop `run_account_action`. Nothing here knows what any
    /// action does: the manifest says which entry point to drive, which fields
    /// must be filled first, which values become which arguments, and which
    /// results land back in which fields.
    pub fn run_account_action_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: AccountActionRequest = from_json("run account action", &request_json)?;
        let (plugin_id, schema) = self.schema_for(&req.adapter_kind)?;
        let action =
            schema
                .action(&req.action_key)
                .cloned()
                .ok_or_else(|| StoreError::InvalidField {
                    field: "action_key".to_string(),
                    detail: "this adapter declares no such action".to_string(),
                })?;

        // Checked here as well as in the frontend: that copy saves a pointless
        // round trip, this one is the gate.
        for requirement in &action.requires {
            let filled = req
                .values
                .get(&requirement.field)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|v| !v.trim().is_empty());
            if !filled {
                return Err(StoreError::InvalidField {
                    field: requirement.field.clone(),
                    detail: requirement.message.clone(),
                });
            }
        }

        let mut args = serde_json::Map::new();
        for (arg, field) in &action.inputs {
            if let Some(value) = req.values.get(field) {
                args.insert(arg.clone(), value.clone());
            }
        }

        let payload: serde_json::Map<String, serde_json::Value> = match action.entry {
            plugin_core::account_schema::AccountActionEntry::Discover => {
                let bytes = self
                    .runtime
                    .block_on(
                        self.plugin_manager
                            .discover(&plugin_id, &serde_json::Value::Object(args).to_string()),
                    )
                    .map_err(|err| StoreError::InvalidField {
                        field: "action_key".to_string(),
                        detail: err.to_string(),
                    })?;
                serde_json::from_slice(&bytes).map_err(|err| StoreError::InvalidField {
                    field: "action_key".to_string(),
                    detail: format!("the plugin's answer was not a JSON object: {err}"),
                })?
            }
        };

        let mut filled = serde_json::Map::new();
        for (field, result_key) in &action.fills {
            if let Some(value) = payload.get(result_key) {
                filled.insert(field.clone(), value.clone());
            }
        }
        to_json(&filled)
    }

    /// Delete an account: unregister its adapter, clear its secrets, and
    /// remove the row. The local account cannot be deleted
    /// ([`StoreError::InvalidField`]).
    pub fn delete_account(&self, account_id: String) -> Result<(), StoreError> {
        self.registry.unregister(&account_id);
        let _ = self.secret_store.delete_all(&account_id);
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        // Whether the deletion travels is a question about the adapter, and the
        // only place the adapter is recorded is the row we are about to remove —
        // so read the kind before it is gone.
        let adapter_kind = repo
            .get(&account_id)
            .map_err(acc_err)?
            .map(|account| account.adapter_kind)
            .ok_or(StoreError::NotFound)?;
        // See the desktop twin: deleting the row this device syncs through
        // strands the pointer and leaves the orchestrator pushing to a target
        // the user thinks is gone.
        if host_core::sync_target::selected_account_id(&UserPrefsRepo::new(&shared)).as_deref()
            == Some(account_id.as_str())
        {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: "this account is the sync target for this device; disconnect it in                          the sync settings before deleting it"
                    .to_string(),
            });
        }
        repo.delete(&account_id).map_err(acc_err)?;
        // This device's half goes with it. The row's removal syncs and this
        // does not — another device deleting the account cannot tell this one
        // where its key file was, so nothing else would ever clean these up.
        let _ = host_core::account_local::forget_all(&UserPrefsRepo::new(&shared), &account_id);
        // Propagate the deletion to other devices (cascades secrets there too).
        append_account_event(
            &self.writer,
            &self.plugin_manager,
            adapter_kind.as_str(),
            SyncEvent::AccountDeleted(IdPayload { id: account_id }),
        );
        Ok(())
    }

    /// Rename an account's display name. Persists the row + syncs the change
    /// (non-secret metadata only). Mirrors the desktop `rename_account`.
    pub fn rename_account_json(&self, id: String, new_name: String) -> Result<String, StoreError> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidField {
                field: "name".to_string(),
                detail: "account name must not be empty".to_string(),
            });
        }
        let shared = self.db.shared();
        let account = AccountsRepo::new(&shared)
            .rename(&id, trimmed)
            .map_err(acc_err)?;
        // An account that never travelled must not receive an update either:
        // no other device has the row to apply it to.
        append_account_event(
            &self.writer,
            &self.plugin_manager,
            account.adapter_kind.as_str(),
            SyncEvent::AccountUpdated(account_payload(&account)),
        );
        to_json(&account)
    }

    /// External accounts whose required keychain secret is absent — the data
    /// behind the credential-repair banner. An account lands here when its
    /// `required_secret_slot` is `Some` and the keychain has nothing in that
    /// slot (a retrieve error, incl. NotFound, counts as missing so the wizard
    /// errs toward letting the user re-authenticate). The local account and
    /// secret-less kinds (iCal) are skipped. Returns a JSON `Account[]`.
    /// Mirrors the desktop `list_accounts_missing_credentials`.
    pub fn list_accounts_missing_credentials_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let all = repo.list().map_err(acc_err)?;
        let mut out = Vec::new();
        for acc in all {
            if acc.id == "local" || acc.adapter_kind == "local" {
                continue;
            }
            // What "connected" means is the ADAPTER's own statement — the
            // required secret fields of the schema it declares, plus a refresh
            // token when it keeps one. Asking the manifest is what keeps this
            // from being a table of kind names that has to be edited (and, as
            // it turned out, forgotten) every time an adapter arrives.
            let slots = host_core::account_setup::required_slots_for_kind(
                &self.plugin_manager,
                acc.adapter_kind.as_str(),
            );
            if slots
                .iter()
                .any(|slot| self.secret_store.retrieve(&acc.id, *slot).is_err())
            {
                out.push(acc);
                continue;
            }
            // Per-ACCOUNT, exactly as on the desktop: a bring-your-own OAuth
            // account whose client secret is on none of this device's paths
            // cannot construct its adapter, however healthy its refresh token
            // looks — the account that synced in before the client secret
            // became syncable.
            let missing_client = host_core::account_setup::schema_for_kind(
                &self.plugin_manager,
                acc.adapter_kind.as_str(),
            )
            .is_some_and(|schema| {
                host_core::account_setup::missing_own_oauth_client_secret(
                    &schema,
                    &acc.config_json,
                    |slot| self.secret_store.retrieve(&acc.id, slot).is_ok(),
                )
            });
            if missing_client {
                out.push(acc);
            }
        }
        to_json(&out)
    }

    /// (Re-)store the secret half of a NON-OAuth account's credentials — the
    /// CalDAV/EWS password or the Vikunja/Todoist API token — then re-register
    /// the adapter so it's live for the rest of the session without a restart.
    /// The keychain slot follows the account kind, matching what the registry
    /// reads back. OAuth accounts (Google/Microsoft Graph) are rejected: they
    /// must re-run the interactive OAuth flow, not paste a secret. Under E2E the
    /// secret also propagates to the user's other devices via the encrypted log.
    /// Mirrors the desktop `set_account_secret`.
    pub fn set_account_secret(&self, account_id: String, secret: String) -> Result<(), StoreError> {
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let account = repo
            .get(&account_id)
            .map_err(acc_err)?
            .ok_or(StoreError::NotFound)?;
        if account.adapter_kind == "local" {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: "the local account has no credential slot".to_string(),
            });
        }
        // Where a pasted credential goes, and whether pasting one is even the
        // right repair, are both the adapter's own statement. The two `match`es
        // this replaces named six adapters between them and answered `Password`
        // for everything they had not heard of — so a new OAuth adapter got a
        // password written into a slot its plugin never reads.
        let Some(schema) = host_core::account_setup::schema_for_kind(
            &self.plugin_manager,
            account.adapter_kind.as_str(),
        ) else {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: format!(
                    "no loaded adapter declares accounts of kind {}",
                    account.adapter_kind.as_str()
                ),
            });
        };
        if host_core::account_setup::signs_in_with_oauth(&schema) {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: format!(
                    "OAuth accounts (kind={}) must reconnect via the OAuth flow",
                    account.adapter_kind.as_str()
                ),
            });
        }
        let Some(slot) = host_core::account_setup::repair_slot(&schema) else {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: format!(
                    "adapter kind {} declares no single credential to replace",
                    account.adapter_kind.as_str()
                ),
            });
        };
        self.secret_store
            .store(&account_id, slot, &secret)
            .map_err(|err| StoreError::Storage {
                detail: format!("failed to store credential: {err}"),
            })?;
        // E2E only: propagate the (re-)entered secret to the user's other
        // devices (gated no-op when E2E is off — credentials stay device-local).
        host_core::credential_sync::emit_credential_set(
            &self.writer,
            &shared,
            &self.plugin_manager,
            &account_id,
            slot,
            &secret,
        );
        // Register so the adapter is live this session. A failure leaves the
        // secret in place — the user can retry without re-typing it.
        self.registry
            .register(&account)
            .map_err(|err| StoreError::Storage {
                detail: format!("adapter registration failed: {err}"),
            })?;
        Ok(())
    }

    // ─── Calendars ───────────────────────────────────────────────────────────

    /// All calendars (local + external + synthetic birthday layers) as a JSON
    /// `CalendarRow[]`, and — as a side effect — primes the registry's
    /// calendar→account route map so the event methods can route. External
    /// calendars go through the SWR cache; birthday layers cover local AND
    /// external contacts (external from the snapshot cache); host-local colour +
    /// name overrides are stamped. Mirrors the desktop `list_calendars` (minus
    /// recurrence-capability resolution, deferred). Callers should list calendars
    /// before event operations — the same ordering the desktop frontend honours.
    pub fn list_calendars_json(&self) -> Result<String, StoreError> {
        // Fetch real calendars (+ their accounts) and the synthetic birthday rows.
        let (mut calendars, accounts, birthday_rows) = self.runtime.block_on(async {
            let local = self.adapter.list_calendars().await.map_err(map_store_err)?;
            for c in &local {
                self.registry.note_calendar_route(&c.id, LOCAL_ID);
            }
            let mut calendars: Vec<Calendar> = Vec::with_capacity(local.len());
            let mut accounts: Vec<String> = Vec::with_capacity(local.len());
            for c in local {
                accounts.push(LOCAL_ID.to_string());
                calendars.push(c);
            }

            // External calendars via SWR, per account: a WARM account serves its
            // cached catalogue instantly (offline, no PROPFIND/folder-walk gating
            // the whole UI); a COLD account serves its RETAINED catalogue when one
            // exists (a live per-account fetch would gate the first paint), and
            // only a never-listed account fetches live. Prime routes from whatever we serve, and
            // background-refresh (which also primes routes + caches) when cold or
            // stale. Mirrors the desktop external_calendars_swr + the mobile
            // cold-fallback. Per-adapter errors are swallowed so one dead account
            // can't blank the whole list.
            for (account, adapter) in self.registry.snapshot_calendar_adapters() {
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::Calendars, "")
                    .ok()
                    .flatten();
                let warm = has_snapshot(&state);
                let stale = is_stale(&state, SWR_TTL_SECS);
                // Cache-only serve — a never-listed account contributes
                // NOTHING until the spawned refresh below lands (its
                // cache-updated push re-lists); the old live per-account
                // listing gated the whole first paint on one cold provider.
                let cals = self.cache.read_calendars(&account).unwrap_or_default();
                for c in &cals {
                    self.registry.note_calendar_route(&c.id, &account);
                }
                if !warm || stale {
                    let adapter_bg = Arc::clone(&adapter);
                    let reg = Arc::clone(&self.registry);
                    let acc = account.clone();
                    spawn_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::Calendars,
                        account.clone(),
                        String::new(),
                        move || async move { adapter_bg.list_calendars().await },
                        move |c, list: &[Calendar]| {
                            for cal in list {
                                reg.note_calendar_route(&cal.id, &acc);
                            }
                            c.replace_calendars(&acc, list)
                        },
                    );
                }
                for c in cals {
                    accounts.push(account.clone());
                    calendars.push(c);
                }
            }

            // Synthetic, read-only birthday calendars (§10.3) — one per contact
            // list (LOCAL + EXTERNAL) that has ≥1 birthday. The shared
            // orchestration reads local contacts in-process + external ones from
            // the snapshot cache (never a network fetch). Events are synthesised
            // on demand in get_events_json. Stamp each id's route so a later
            // lookup resolves; no overrides apply to synthetic calendars.
            let mut birthday_rows: Vec<CalendarRow> = Vec::new();
            for (cal, account_id) in host_core::birthdays::list_birthday_calendars(
                &self.adapter,
                &self.registry,
                &self.cache,
            )
            .await
            {
                self.registry.note_calendar_route(&cal.id, &account_id);
                birthday_rows.push(CalendarRow::new(
                    cal,
                    account_id,
                    RecurrenceCapabilities::default(),
                ));
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

        // Snapshot account_id → adapter_kind once so each row's recurrence-caps
        // lookup is a cheap map hit (mirrors task_lists_json + the desktop). A
        // read failure degrades to "every account looks local" → permissive caps.
        let shared = self.db.shared();
        let account_kinds: std::collections::HashMap<String, AdapterKind> =
            AccountsRepo::new(&shared)
                .list()
                .map(|accs| accs.into_iter().map(|a| (a.id, a.adapter_kind)).collect())
                .unwrap_or_default();
        let mut rows: Vec<CalendarRow> = calendars
            .into_iter()
            .zip(accounts)
            .map(|(c, account)| {
                let caps = self.recurrence_caps_for_account(&account, &account_kinds);
                CalendarRow::new(c, account, caps)
            })
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
        to_json(&CalendarRow::new(
            created,
            LOCAL_ID.to_string(),
            RecurrenceCapabilities::default(),
        ))
    }

    /// Delete a local calendar (its events cascade away). Mirrors the desktop
    /// local-only `delete_calendar`.
    pub fn delete_calendar(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_calendar(&id).map_err(map_store_err)?;
        // The calendar's events went with it, so no group can go on counting
        // them.
        self.forget_calendar_groupings(&id);
        self.writer
            .append(SyncEvent::CalendarDeleted(IdPayload { id }));
        Ok(())
    }

    // ─── Events ──────────────────────────────────────────────────────────────

    /// Events in `calendar_id` overlapping `[start, end]`, as a JSON `Event[]`.
    /// A synthetic birthday calendar id is intercepted first (§10.3 — its all-day
    /// events are synthesised from the underlying contact list's birthdays,
    /// local in-process + external from the snapshot cache). A LOCAL calendar is
    /// a direct read; an EXTERNAL one is stale-while-revalidate (serve the cached
    /// snapshot — empty on a never-warmed calendar — and background-refresh),
    /// mirroring the desktop `get_events`.
    ///
    /// The local adapter currently returns rows whose stored start/end
    /// intersect the range (RRULE occurrence expansion is its own later phase),
    /// so a recurring master is returned only when its stored span overlaps.
    pub fn get_events_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: EventRangeRequest = from_json("request", &request_json)?;
        let range = DateRange::new(req.start, req.end);

        // Birthday calendars are synthesised, not stored: derive their events
        // from the underlying contact list's birthdays — LOCAL contacts read
        // in-process, EXTERNAL ones from the snapshot cache (never a network
        // fetch), via the shared orchestration.
        if host_core::birthdays::is_birthday_calendar_id(&req.calendar_id) {
            let events = self
                .runtime
                .block_on(async {
                    host_core::birthdays::synthesise_birthday_events(
                        &self.adapter,
                        &self.registry,
                        &self.cache,
                        &req.calendar_id,
                        range,
                    )
                    .await
                })
                .unwrap_or_default();
            return to_json(&events);
        }

        match self.route(&req.calendar_id)? {
            // LOCAL: a direct read (the local store isn't cached).
            None => {
                let events = self
                    .runtime
                    .block_on(async { self.adapter.get_events(&req.calendar_id, range).await })
                    .map_err(map_store_err)?;
                to_json(&events)
            }
            // EXTERNAL: stale-while-revalidate. A WARM container (a snapshot
            // exists) serves the cached rows for the range instantly (never
            // blocks on the network, works offline) and queues a background
            // refresh when the snapshot is stale. A COLD container serves the
            // RETAINED rows when it has any — an invalidate (after an external
            // write) deliberately keeps them as the fallback, so the post-save
            // reload paints instantly with the pre-write rows and the
            // background refresh swaps in the truth via the cache-updated push
            // (desktop parity). A NEVER-warmed container (no rows at all —
            // fresh install / newly added account) serves EMPTY and lets the
            // spawned self-warm below fill the view via that same push: the
            // old blocking LIVE read here ran a seconds-long provider fetch
            // on the serial native queue, stalling EVERY other read behind
            // one cold calendar at exactly the moment the whole app is
            // trying to first-paint. Whether to self-warm (stale, OR a
            // coverage miss outside the brief cooldown that prevents the
            // cold-cache refresh -> re-read feedback loop) is the SHARED
            // `event_self_warm_needed`, identical to the desktop get_events.
            Some(ext) => {
                let account = self
                    .registry
                    .account_for_calendar(&req.calendar_id)
                    .unwrap_or_else(|| LOCAL_ID.to_string());
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::Events, &req.calendar_id)
                    .ok()
                    .flatten();
                let mut events = self
                    .cache
                    .read_events(&account, &req.calendar_id, range)
                    .unwrap_or_default();
                // Colour: map a colour-capable provider's native color_hex back
                // to a label FIRST, then stamp host-local overrides (which skip
                // native-coloured events) — the desktop ordering, so a provider
                // colour always wins.
                for ev in events.iter_mut() {
                    if let Some(hex) = ev.color_hex.clone() {
                        if let Ok(Some(label)) = self.adapter.match_hex_to_label(&hex) {
                            ev.color_label = Some(ColorLabelId(label));
                        }
                    }
                }
                let shared = self.db.shared();
                apply_color_to_events(&OverridesRepo::new(&shared), &mut events);
                if event_self_warm_needed(&state, range) {
                    let cache_bg = Arc::clone(&self.cache);
                    let ext_bg = Arc::clone(&ext);
                    let acc = account.clone();
                    let cal = req.calendar_id.clone();
                    spawn_item_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::Events,
                        account,
                        req.calendar_id.clone(),
                        move || async move {
                            refresh_events(&cache_bg, ext_bg.as_ref(), &acc, &cal, range).await
                        },
                    );
                }
                // Hide cancelled events when the user turned OFF the (synced,
                // default-on) show-cancelled setting. Only external providers
                // surface cancelled rows; reminders for them are suppressed
                // separately, core-side, regardless of this toggle.
                if !show_cancelled_events(&self.db.shared()) {
                    events.retain(|e| !e.cancelled);
                }
                to_json(&events)
            }
        }
    }

    /// One event by id as JSON (`Event` or `null`). `calendar_id` routes the
    /// lookup: a LOCAL calendar (or an absent/unknown id) reads the stored row;
    /// an EXTERNAL one — which has no by-id adapter fetch — is resolved from the
    /// SWR snapshot cache by account. Passing the owning calendar is what lets
    /// the editor open an external event (the local store has no row for it).
    pub fn get_event_by_id_json(
        &self,
        id: String,
        calendar_id: Option<String>,
    ) -> Result<String, StoreError> {
        // Route by the owning calendar when the caller knows it. A LOCAL calendar
        // (or an absent/unknown calendar_id) reads the stored row directly. An
        // EXTERNAL calendar has no by-id adapter fetch (CalendarFeature exposes
        // none), so look the event up in the SWR snapshot cache — warm after the
        // list read that necessarily preceded this edit. THIS is what lets the
        // editor open an external event at all: without it the lookup was
        // local-only, so an external event opened EMPTY and a save then created a
        // duplicate. A cold cache (rare: a deep-link before any list read) falls
        // back to `null`, exactly as before. `id` is the series master id (the
        // editor passes seriesIdOf), which is what the cache stores (un-expanded).
        // Gate on locality (not `route`, which would Err on a non-live external
        // adapter): the cache read needs only the resolved ACCOUNT, so an
        // external event resolves even when its adapter isn't currently live. An
        // unknown id (route map not yet primed) degrades to the local lookup →
        // `null`, exactly as before.
        let local = match &calendar_id {
            Some(cid) => self.is_local_calendar(cid),
            None => true,
        };
        let event = if local {
            self.adapter.get_event_by_id(&id).map_err(map_store_err)?
        } else {
            let cid = calendar_id.as_deref().unwrap_or_default();
            let account = self
                .registry
                .account_for_calendar(cid)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            // No range on a by-id lookup → scan the whole cached window. The
            // 4-digit-year bounds keep the cache's lexicographic RFC-3339
            // comparison valid (chrono's `+`/`-` year prefixes for MIN/MAX_UTC
            // would not sort against stored timestamps).
            let whole = DateRange::new(
                "0001-01-01T00:00:00Z"
                    .parse()
                    .expect("valid lower wide-range bound"),
                "9999-12-31T23:59:59Z"
                    .parse()
                    .expect("valid upper wide-range bound"),
            );
            let mut events = self
                .cache
                .read_events(&account, cid, whole)
                .unwrap_or_default();
            // Resolve colour the same way the list read does (native
            // color_hex → label, then host-local overrides) so the editor's
            // colour picker seeds the effective value.
            for ev in events.iter_mut() {
                if let Some(hex) = ev.color_hex.clone() {
                    if let Ok(Some(label)) = self.adapter.match_hex_to_label(&hex) {
                        ev.color_label = Some(ColorLabelId(label));
                    }
                }
            }
            let shared = self.db.shared();
            apply_color_to_events(&OverridesRepo::new(&shared), &mut events);
            events.into_iter().find(|e| e.id == id)
        };
        to_json(&event)
    }

    /// Local full-text search (FTS5) over events + tasks, as a JSON
    /// `SearchResults { events, tasks }`. Mirrors the desktop `search` command's
    /// LOCAL half — the engine already lives in `adapter-local`. The external
    /// snapshot-cache half needs the SWR cache the mobile host lacks, so it's
    /// omitted (a known parity gap). `filters_json` is a JSON `SearchFilters`, or
    /// `""` for no filters (default = both kinds, no restrictions).
    //
    // External snapshot-cache half (§13.1 — search covers every locally cached
    // item): the cache now exists, so the FTS mirrors are merged in below, best
    // effort. (Doc comment left verbatim so the UniFFI per-method checksum stays
    // identical — no binding regen; see reference_uniffi_docstring_checksum.)
    pub fn search_json(&self, query: String, filters_json: String) -> Result<String, StoreError> {
        let filters: SearchFilters = if filters_json.trim().is_empty() {
            SearchFilters::default()
        } else {
            from_json("filters", &filters_json)?
        };
        let mut results = self
            .adapter
            .search(&query, &filters)
            .map_err(map_store_err)?;

        // EXTERNAL snapshot-cache half. Best-effort: an error is swallowed so a
        // stale cache can't sink the whole search — the local results are always
        // returned. Empty MATCH string → no cache hits (the FTS helpers no-op).
        let fts = prepare_fts_query(&query);
        if !fts.is_empty() {
            if let Ok(events) = self.cache.search_events_fts(&fts, &filters) {
                results.events.extend(events);
            }
            if let Ok(tasks) = self.cache.search_tasks_fts(&fts, &filters) {
                results.tasks.extend(tasks);
            }
        }
        to_json(&results)
    }

    /// Cross-account contact search: local hits first, then external (each
    /// adapter caps its own result; external errors are swallowed per-adapter).
    /// Returns a JSON `Contact[]`. Mirrors the desktop `search_contacts` — for
    /// the ContactsScreen filter + the attendee typeahead.
    pub fn search_contacts_json(&self, query: String) -> Result<String, StoreError> {
        let contacts = self
            .runtime
            .block_on(async {
                let local = self.adapter.search_contacts(&query).await?;
                let mut external = self.registry.search_external_contacts(&query).await;
                let mut out = local;
                out.append(&mut external);
                Ok::<_, cal_core::Error>(out)
            })
            .map_err(map_store_err)?;
        to_json(&contacts)
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
        self.invalidate_events_cache(&created.calendar_id);
        to_json(&created)
    }

    /// Update an event; `previous_calendar_id` (the calendar the editor loaded
    /// the event FROM) lets the bridge detect a cross-calendar MOVE — the
    /// editor's calendar picker doubles as a move gesture. Returns the resulting
    /// `Event` as JSON. Mirrors the desktop `update_event`.
    ///
    /// A move to an EXTERNAL target can't be a plain in-place PUT: it would PUT
    /// to a resource that doesn't exist on the target while carrying the source's
    /// ETag in If-Match, which a provider like iCloud rejects with 412 (the
    /// precondition can never be met) — the move would silently fail. So a move
    /// reduces to create-on-target + best-effort-delete-from-source, creating
    /// FIRST so a half-failed move leaves a recoverable duplicate rather than an
    /// empty hole. A local↔local move stays a single SQL `UPDATE` on the
    /// `calendar_id` column. (No cache-driven colour-hex resolution here — cal-ffi
    /// has no read cache, same as `create_event_json`; colour rides `color_label`/
    /// `color_hex` straight through and the client applies any override after.)
    pub fn update_event_json(
        &self,
        event_json: String,
        previous_calendar_id: Option<String>,
    ) -> Result<String, StoreError> {
        let event: Event = from_json("event", &event_json)?;

        let target_local = self.is_local_calendar(&event.calendar_id);
        let is_move = previous_calendar_id
            .as_deref()
            .map(|prev| prev != event.calendar_id)
            .unwrap_or(false);

        if is_move {
            let previous = previous_calendar_id.expect("checked above");
            let source_local = self.is_local_calendar(&previous);

            // Local↔Local: the LocalAdapter handles the calendar_id change as a
            // single SQL UPDATE — no resource-URL gymnastics, so there's nothing
            // to gain from the two-call dance.
            if source_local && target_local {
                let moved_to = event.calendar_id.clone();
                let updated = self
                    .runtime
                    .block_on(async { self.adapter.update_event(event).await })
                    .map_err(map_store_err)?;
                // A move is not an unlinking: the copy still means the same
                // appointment, it just lives elsewhere now. Membership is keyed
                // by (calendar, event), so it has to be carried across.
                self.relocate_event_grouping(&previous, &updated.id, &moved_to, &updated.id);
                if let Ok(fields) = serde_json::to_value(&updated) {
                    self.writer.append(SyncEvent::EventUpdated(EventPayload {
                        id: updated.id.clone(),
                        fields,
                    }));
                }
                self.invalidate_events_cache(&updated.calendar_id);
                return to_json(&updated);
            }

            // Cross-adapter move (at least one external side). Create on the
            // target FIRST; the source delete is best-effort (logged, not bubbled)
            // so a failed cleanup leaves a resolvable duplicate, not data loss.
            let new_payload = NewEvent {
                // A move re-creates at the target; the organizer-notify intent
                // isn't carried through this path (matches the desktop).
                send_invitations: false,
                title: event.title.clone(),
                description: event.description.clone(),
                location: event.location.clone(),
                start: event.start,
                end: event.end,
                all_day: event.all_day,
                recurrence: event.recurrence.clone(),
                color_label: event.color_label.clone(),
                color_hex: event.color_hex.clone(),
                reminders: event.reminders.clone(),
                sound: event.sound.clone(),
                attendees: event.attendees.clone(),
            };
            let target_calendar_id = event.calendar_id.clone();
            let source_event_id = event.id.clone();
            let created = self.runtime.block_on(async {
                let created = match self.route(&target_calendar_id)? {
                    None => self
                        .adapter
                        .create_event(&target_calendar_id, new_payload)
                        .await
                        .map_err(map_store_err)?,
                    Some(ext) => ext
                        .create_event(&target_calendar_id, new_payload)
                        .await
                        .map_err(map_store_err)?,
                };
                // Delete from the source. A move is NOT a cancellation — the
                // event still exists at the target — so never email attendees.
                // Best-effort: the create already succeeded, so the cleanup must
                // NEVER abort the move. A non-routable source (the account was
                // logged out / removed between the editor opening and the save)
                // is treated as "no cleanup needed — the create on the target
                // stands", mirroring the desktop; so resolve the route WITHOUT
                // `?` (an erroring `?` here would return Err after a successful
                // create → a spurious failure + a duplicate on retry + skipped
                // sync-log appends). cal-ffi has no logger, so a delete error is
                // swallowed too; the user resolves any duplicate manually.
                match self.route(&previous) {
                    Ok(None) => {
                        let _ = self.adapter.delete_event(&source_event_id, false).await;
                    }
                    Ok(Some(ext)) => {
                        let _ = ext.delete_event(&source_event_id, false).await;
                    }
                    // Source account gone — nothing to clean up; the move stands.
                    Err(_) => {}
                }
                Ok::<_, StoreError>(created)
            })?;

            // Carry the grouping over to the copy at the target: a
            // cross-adapter move re-creates the event, so it lives under a new
            // id now. The source delete here goes straight to the adapter, so
            // it does not run the membership cleanup `delete_event` does —
            // this is what keeps the group whole across a move.
            self.relocate_event_grouping(
                &previous,
                &source_event_id,
                &target_calendar_id,
                &created.id,
            );

            // Each LOCAL side emits its own event-log entry; external sides stay
            // silent (the provider's own sync mesh propagates the change).
            if target_local {
                if let Ok(fields) = serde_json::to_value(&created) {
                    self.writer.append(SyncEvent::EventCreated(EventPayload {
                        id: created.id.clone(),
                        fields,
                    }));
                }
            }
            if source_local {
                self.writer.append(SyncEvent::EventDeleted(IdPayload {
                    id: source_event_id,
                }));
            }
            // A move touches both ends — invalidate the source + the target so
            // neither serves a stale snapshot (mirrors the desktop's both-ends
            // invalidate).
            self.invalidate_events_cache(&previous);
            self.invalidate_events_cache(&target_calendar_id);
            return to_json(&created);
        }

        // Plain in-place update — no calendar change.
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
        self.invalidate_events_cache(&updated.calendar_id);
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
                .append(SyncEvent::EventDeleted(IdPayload { id: id.clone() }));
        }
        if let Some(cid) = calendar_id.as_deref() {
            self.forget_event_grouping(cid, &id);
            self.invalidate_events_cache(cid);
        }
        Ok(())
    }

    /// Append one occurrence's date to a recurring event's EXDATE list so the
    /// expansion engine skips it — the "delete / edit THIS occurrence only" flow
    /// (the master row's start/title/… are untouched). `occurrence` is the
    /// occurrence's RFC-3339 instant; `calendar_id` routes (omitted → local). A
    /// LOCAL event re-reads + logs EventUpdated so the exclusion syncs; an
    /// external event self-syncs via the provider. Mirrors the desktop
    /// `add_event_exdate` (minus its cache-invalidate + scheduler bits).
    pub fn add_event_exdate_json(
        &self,
        id: String,
        occurrence: String,
        calendar_id: Option<String>,
        send_cancellations: bool,
    ) -> Result<(), StoreError> {
        let occ = chrono::DateTime::parse_from_rfc3339(occurrence.trim())
            .map_err(|e| StoreError::InvalidField {
                field: "occurrence".to_string(),
                detail: format!("not an RFC-3339 instant: {e}"),
            })?
            .with_timezone(&chrono::Utc);
        let route = match calendar_id.as_deref() {
            Some(cid) => self.route(cid)?,
            None => None,
        };
        match route {
            None => {
                // LocalAdapter's sync inherent add_event_exdate rewrites the
                // master row's recurrence.exceptions; re-read + log so it syncs.
                self.adapter
                    .add_event_exdate(&id, occ)
                    .map_err(map_store_err)?;
                if let Ok(Some(refreshed)) = self.adapter.get_event_by_id(&id) {
                    if let Ok(fields) = serde_json::to_value(&refreshed) {
                        self.writer.append(SyncEvent::EventUpdated(EventPayload {
                            id: id.clone(),
                            fields,
                        }));
                    }
                }
            }
            Some(ext) => {
                // `send_cancellations` = the organizer chose "cancel this
                // occurrence + notify attendees" (vs a silent local skip).
                self.runtime
                    .block_on(async { ext.add_event_exdate(&id, occ, send_cancellations).await })
                    .map_err(map_store_err)?;
            }
        }
        if let Some(cid) = calendar_id.as_deref() {
            self.invalidate_events_cache(cid);
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
    // `Unsupported`); cross-list MOVES aren't detected here (no previous_list_id —
    // the Move/Copy flow handles those); the reminder-scheduler kick is the
    // desktop worker's job (mobile reschedules via its own JS scheduler). External
    // recurring tasks still get no host-ASSIGNED series_id (the spawn dedups via
    // the snapshot cache instead). NO LONGER deferred: external on-demand/backlog
    // recurring-task resurface IS handled (`spawn_external_on_demand`, wired into
    // the external `update_task` branch), and external mutations invalidate the
    // SWR cache (`invalidate_*_cache`).

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
        // External task lists via SWR, per account — same shape as the calendar
        // listing (see list_calendars_json): a WARM account serves its cached
        // catalogue instantly (offline, no provider round-trip gating the UI); a
        // COLD account serves its RETAINED catalogue when one exists (a live
        // per-account fetch would gate the tasks tab's first load), and only a
        // never-listed account fetches live.
        // Prime routes from whatever we serve, and background-refresh (which also
        // primes routes + caches) when cold or stale. Per-adapter errors are
        // swallowed so one dead account can't blank the whole list.
        let external = self.runtime.block_on(async {
            let mut out: Vec<TaskList> = Vec::new();
            for (account, adapter) in self.registry.snapshot_task_adapters() {
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::TaskLists, "")
                    .ok()
                    .flatten();
                let warm = has_snapshot(&state);
                let stale = is_stale(&state, SWR_TTL_SECS);
                // Cache-only serve — a never-listed account contributes
                // NOTHING until the spawned refresh below lands (the tasks
                // tab gates its FIRST load on this listing, so the old live
                // per-account fetch here was exactly the app-start "Laden…"
                // stall). Mirrors list_calendars_json.
                let lists = self.cache.read_task_lists(&account).unwrap_or_default();
                for l in &lists {
                    self.registry.note_task_list_route(&l.id, &account);
                }
                if !warm || stale {
                    let adapter_bg = Arc::clone(&adapter);
                    let reg = Arc::clone(&self.registry);
                    let acc = account.clone();
                    spawn_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::TaskLists,
                        account.clone(),
                        String::new(),
                        move || async move { adapter_bg.list_task_lists().await },
                        move |c, list: &[TaskList]| {
                            for l in list {
                                reg.note_task_list_route(&l.id, &acc);
                            }
                            c.replace_task_lists(&acc, list)
                        },
                    );
                }
                out.extend(lists);
            }
            out
        });

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
        let mut meta: Vec<(String, TaskCapabilities, RecurrenceCapabilities)> =
            Vec::with_capacity(local.len() + external.len());
        for l in local {
            meta.push((
                LOCAL_ID.to_string(),
                local_task_capabilities(),
                RecurrenceCapabilities::default(),
            ));
            lists.push(l);
        }
        for l in external {
            let account_id = self
                .registry
                .account_for_task_list(&l.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            let task_capabilities = self.task_caps_for_account(&account_id, &account_kinds);
            let recurrence_capabilities =
                self.recurrence_caps_for_account(&account_id, &account_kinds);
            meta.push((account_id, task_capabilities, recurrence_capabilities));
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
            .map(
                |(inner, (account_id, task_capabilities, recurrence_capabilities))| TaskListRow {
                    inner,
                    account_id,
                    task_capabilities,
                    recurrence_capabilities,
                },
            )
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
            Some(ext) => {
                // Capture the owning account BEFORE the delete — the provider
                // round-trip may self-sync and prune the registry mapping, so
                // re-resolving afterward (as the invalidate_* helpers do) could
                // fall back to LOCAL_ID and skip the real account's rows.
                let account = self
                    .registry
                    .account_for_task_list(&id)
                    .unwrap_or_else(|| LOCAL_ID.to_string());
                self.runtime
                    .block_on(async { ext.delete_task_list(&id).await })
                    .map_err(map_store_err)?;
                // The list is gone — drop its cached listing / tasks / sections
                // snapshots (against the captured account) so nothing stale is
                // served if the provider ever reuses the id. Mirrors the desktop
                // delete_task_list write-through (commands/tasks.rs) plus the
                // mobile-only Sections scope. Best-effort: a failed invalidate
                // only costs a stale read, never the delete's correctness.
                let _ = self.cache.invalidate(&account, SyncScope::TaskLists, "");
                let _ = self.cache.invalidate(&account, SyncScope::Tasks, &id);
                let _ = self.cache.invalidate(&account, SyncScope::Sections, &id);
                Ok(())
            }
        }
    }

    /// Tasks in a list as a JSON array (`cal_core::Task[]`), routed to the list's
    /// owning account (local store or external provider).
    pub fn tasks_json(&self, list_id: String) -> Result<String, StoreError> {
        match self.route_task_list(&list_id)? {
            // LOCAL: a direct read (the local store isn't cached).
            None => {
                let tasks = self
                    .adapter
                    .get_tasks_sync(&list_id)
                    .map_err(map_store_err)?;
                to_json(&tasks)
            }
            // EXTERNAL: stale-while-revalidate with a mobile cold-fallback — see
            // get_events_json for the full rationale. A WARM list (a snapshot
            // exists) serves the cached rows instantly + queues a background
            // refresh when stale; a COLD one serves the RETAINED rows when it
            // has any (an invalidate keeps them as the fallback; the refresh
            // swaps in the truth), and a never-warmed list serves EMPTY — the
            // spawned refresh fills the view via the cache-updated push. The
            // old blocking LIVE read here stalled the whole serial native
            // queue behind one cold provider fetch at first-paint time (see
            // get_events_json). The refresh is gated on stale|cold (not
            // coverage). Mirrors the desktop tasks read.
            Some(ext) => {
                let account = self
                    .registry
                    .account_for_task_list(&list_id)
                    .unwrap_or_else(|| LOCAL_ID.to_string());
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::Tasks, &list_id)
                    .ok()
                    .flatten();
                let warm = has_snapshot(&state);
                let stale = is_stale(&state, SWR_TTL_SECS);
                let tasks = self
                    .cache
                    .read_tasks(&account, &list_id)
                    .unwrap_or_default();
                if !warm || stale {
                    let cache_bg = Arc::clone(&self.cache);
                    let ext_bg = Arc::clone(&ext);
                    let acc = account.clone();
                    let list = list_id.clone();
                    spawn_item_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::Tasks,
                        account,
                        list_id.clone(),
                        move || async move {
                            refresh_tasks(&cache_bg, ext_bg.as_ref(), &acc, &list).await
                        },
                    );
                }
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
                // Invalidate-only, NOT write-through: a created row's id may
                // not match the read path's later id for the same task
                // (CalDAV create → bare uid, reads → `{href}|{uid}`), so
                // planting it would produce a persistent duplicate once the
                // delta brings the composite-id row. The new task surfaces
                // on the next refresh instead.
                self.invalidate_tasks_cache(&task.list_id);
                to_json(&task)
            }
        }
    }

    /// Update a task from a JSON `cal_core::Task`, routed by its `list_id`.
    /// `previous_list_id` is the list the editor loaded the task FROM; when it
    /// differs from the task's `list_id` the save is a cross-list MOVE — the
    /// list picker doubles as a "move to another list" gesture. Returns the
    /// resulting `Task` as JSON. Mirrors the desktop `update_task`.
    ///
    /// A move to an EXTERNAL list can't be an in-place PATCH (it would hit the
    /// wrong resource: a CalDAV VTODO at the old URL → 412, Google Tasks
    /// `tasks.patch` against the wrong tasklist → 404). So a move reduces to
    /// create-on-target + best-effort-delete-from-source, creating FIRST so a
    /// half-failed move leaves a recoverable duplicate rather than nothing. A
    /// local↔local move stays a single SQL `UPDATE` on the `list_id` column.
    /// The new task gets a fresh adapter-assigned id; the client's refetch on
    /// editor-close surfaces it (no old→new id translation needed).
    ///
    /// In-place LOCAL: a single SQL UPDATE + `TaskUpdated` (completing a recurring
    /// task spawns its next instance locally; the peer's applier re-runs the
    /// spawner deduped on `series_id`, so only `TaskUpdated` crosses). In-place
    /// EXTERNAL: routed to the provider (no event log; it self-syncs). Deferred
    /// for external (cache-dependent, documented gaps): the host-side series_id
    /// assignment + on-demand next-instance spawn for external recurring tasks.
    pub fn update_task_json(
        &self,
        task_json: String,
        previous_list_id: Option<String>,
    ) -> Result<String, StoreError> {
        let mut task: cal_core::Task = from_json("task", &task_json)?;

        // DESIGN §9.12: a task that GAINS on-demand recurrence via an edit (not
        // just at creation) needs a stable series_id, or the idempotent spawner
        // has nothing to dedup on and re-completing it spawns a duplicate. Assign
        // one host-side before any routing, mirroring the desktop update_task.
        // The local adapter's update_task_sync writes series_id verbatim and
        // relies on the host having assigned it (create_task_sync's
        // ensure_series_id covers the external→local move, but a local↔local move
        // and a plain in-place local edit reach update_task_sync directly). Plain
        // scheduled rules get none — the provider owns those.
        if task.series_id.is_none()
            && task
                .recurrence
                .as_ref()
                .is_some_and(cal_core::recurrence_needs_extras)
        {
            task.series_id = Some(uuid::Uuid::new_v4().to_string());
        }

        let target_local = self.is_local_task_list(&task.list_id);
        let is_move = previous_list_id
            .as_deref()
            .map(|prev| prev != task.list_id)
            .unwrap_or(false);

        if is_move {
            let previous = previous_list_id.expect("checked above");
            let source_local = self.is_local_task_list(&previous);

            // Local↔Local: the LocalAdapter does the move as a single SQL UPDATE
            // on the list_id column — no create+delete dance needed.
            if source_local && target_local {
                let updated = self.adapter.update_task_sync(task).map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&updated) {
                    self.writer.append(SyncEvent::TaskUpdated(EventPayload {
                        id: updated.id.clone(),
                        fields,
                    }));
                }
                // A move touches both ends; the helpers no-op for the local
                // account (no cache row), but stay consistent with the
                // cross-adapter branch + the desktop's both-ends invalidate.
                self.invalidate_tasks_cache(&previous);
                self.invalidate_tasks_cache(&updated.list_id);
                return to_json(&updated);
            }

            // Cross-adapter move (at least one external side). The new task lands
            // at the top of the target list with no section (sections belong to
            // the source list, hence section_id: None). Create on the target
            // FIRST; the source delete is best-effort.
            let new_payload = cal_core::NewTask {
                assignees: Vec::new(),
                title: task.title.clone(),
                description: task.description.clone(),
                status: task.status,
                priority: task.priority,
                effort: task.effort,
                scheduled_date: task.scheduled_date,
                scheduled_time: task.scheduled_time,
                scheduled_end_time: task.scheduled_end_time,
                deadline_date: task.deadline_date,
                deadline_time: task.deadline_time,
                deadline_reminder_days: task.deadline_reminder_days,
                recurrence: task.recurrence.clone(),
                resurface_date: task.resurface_date,
                series_id: task.series_id.clone(),
                parent_id: task.parent_id.clone(),
                section_id: None,
                color_label: task.color_label.clone(),
                reminders: task.reminders.clone(),
                sound: task.sound.clone(),
            };
            let target_list_id = task.list_id.clone();
            let source_task_id = task.id.clone();

            let created = match self.route_task_list(&target_list_id)? {
                None => self
                    .adapter
                    .create_task_sync(&target_list_id, new_payload)
                    .map_err(map_store_err)?,
                Some(ext) => self
                    .runtime
                    .block_on(async { ext.create_task(&target_list_id, new_payload).await })
                    .map_err(map_store_err)?,
            };

            // Delete from the source. Best-effort: the create already succeeded,
            // so the cleanup must NEVER abort the move. A non-routable source
            // (the account was logged out / removed between the editor opening
            // and the save) is treated as "no cleanup needed — the create on the
            // target stands", mirroring the desktop; so resolve the route WITHOUT
            // `?` (an erroring `?` here would return Err after a successful create
            // → a spurious failure + a duplicate on retry + skipped sync-log
            // appends). cal-ffi has no logger, so a delete error is swallowed too.
            match self.route_task_list(&previous) {
                Ok(None) => {
                    let _ = self.adapter.delete_task_sync(&source_task_id);
                }
                Ok(Some(ext)) => {
                    let _ = self
                        .runtime
                        .block_on(async { ext.delete_task(&source_task_id).await });
                }
                // Source account gone — nothing to clean up; the move stands.
                Err(_) => {}
            }

            // Each LOCAL side emits its own event-log entry; external sides stay
            // silent (the provider's own sync mesh propagates the change).
            if target_local {
                if let Ok(fields) = serde_json::to_value(&created) {
                    self.writer.append(SyncEvent::TaskCreated(EventPayload {
                        id: created.id.clone(),
                        fields,
                    }));
                }
            }
            if source_local {
                self.writer.append(SyncEvent::TaskDeleted(IdPayload {
                    id: source_task_id.clone(),
                }));
            }
            // The move's SOURCE loses the moved-away row via a removal
            // write-through so its retained snapshot can't resurrect it. The
            // TARGET is a create → invalidate-only (a created row's id may
            // not match the read path's later id — CalDAV bare uid vs
            // `{href}|{uid}` — so a write-through would plant a duplicate);
            // the moved task surfaces on the next refresh. Local sides are
            // invalidate-only regardless (the local store isn't cached).
            if source_local {
                self.invalidate_tasks_cache(&previous);
            } else {
                self.write_through_task_removal(&previous, &source_task_id);
            }
            self.invalidate_tasks_cache(&target_list_id);
            return to_json(&created);
        }

        // Plain in-place update — no list change.
        match self.route_task_list(&task.list_id)? {
            None => {
                // The local adapter spawns the next on-demand instance and
                // returns it; emit a `task.created` for that row too, or the
                // recurrence never reaches other devices until the next
                // snapshot/backfill.
                let (updated, spawned) = self
                    .adapter
                    .update_task_with_spawn(task)
                    .map_err(map_store_err)?;
                if let Ok(fields) = serde_json::to_value(&updated) {
                    self.writer.append(SyncEvent::TaskUpdated(EventPayload {
                        id: updated.id.clone(),
                        fields,
                    }));
                }
                // After the completion's TaskUpdated, so receivers apply
                // "completed" before "next instance created".
                if let Some(next) = spawned {
                    if let Ok(fields) = serde_json::to_value(&next) {
                        self.writer.append(SyncEvent::TaskCreated(EventPayload {
                            id: next.id.clone(),
                            fields,
                        }));
                    }
                }
                to_json(&updated)
            }
            Some(ext) => {
                // Capture the completion intent before `task` moves into the
                // update — an external on-demand/backlog recurring task that just
                // completed needs Aperio to spawn its next instance (§9.12).
                let completed = task.clone();
                let updated = self.runtime.block_on(async {
                    let updated = ext.update_task(task).await.map_err(map_store_err)?;
                    let account = self
                        .registry
                        .account_for_task_list(&completed.list_id)
                        .unwrap_or_else(|| LOCAL_ID.to_string());
                    // A DEVICE reminder (iOS/Android) owns its own recurrence
                    // lifecycle: the OS spawns the next turn and keeps the
                    // completed history itself. Recording Aperio's terminal
                    // completion snapshot back via ext.create_task would write a
                    // DUPLICATE reminder into the device store, so skip the
                    // external-recurrence reconciliation for device accounts
                    // (mirrors the device-account skip in the reminder enumerator).
                    if !self.is_device_account(&account) {
                        host_core::tasks::record_external_recurrence_completion(
                            ext.as_ref(),
                            &self.cache,
                            &account,
                            &completed,
                        )
                        .await;
                    }
                    Ok::<_, StoreError>(updated)
                })?;
                // Write-through: the retained snapshot must reflect the edit
                // immediately — a completed check-off used to stay visibly
                // open until the background refresh landed.
                self.write_through_task(&updated);
                to_json(&updated)
            }
        }
    }

    /// Whether `account_id` belongs to a device-calendar/-reminder account (the
    /// phone's own EventKit / CalendarProvider store). Used to skip Aperio-side
    /// recurrence reconciliation the OS already owns.
    fn is_device_account(&self, account_id: &str) -> bool {
        let shared = self.db.shared();
        AccountsRepo::new(&shared)
            .list()
            .unwrap_or_default()
            .into_iter()
            .any(|a| {
                a.id == account_id
                    && a.adapter_kind == host_core::builtin_adapters::device_calendar_kind()
            })
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
            Some(ext) => {
                self.runtime
                    .block_on(async { ext.delete_task(&id).await })
                    .map_err(map_store_err)?;
                if let Some(lid) = list_id.as_deref() {
                    // Write-through removal: the retained snapshot would
                    // otherwise resurrect the deleted row on the next read.
                    self.write_through_task_removal(lid, &id);
                }
                Ok(())
            }
        }
    }

    /// Sections of a list as a JSON array (`cal_core::Section[]`), routed to the
    /// list's owning account.
    pub fn sections_json(&self, list_id: String) -> Result<String, StoreError> {
        match self.route_task_list(&list_id)? {
            // LOCAL: a direct read (the local store isn't cached).
            None => {
                let sections = self
                    .adapter
                    .list_sections_sync(&list_id)
                    .map_err(map_store_err)?;
                to_json(&sections)
            }
            // EXTERNAL: stale-while-revalidate with a mobile cold-fallback —
            // mirrors tasks_json. A WARM list (a section snapshot exists) serves
            // the cached sections instantly + queues a background refresh when
            // stale; a COLD one serves the RETAINED rows when it has any (an
            // invalidate keeps them; the refresh swaps in the truth), and only a
            // never-warmed list does a LIVE `list_sections` read. Section reads
            // used to ALWAYS hit the provider live (the unconditional
            // `list_sections` below); the cache stops that on every day/week open.
            Some(ext) => {
                let account = self
                    .registry
                    .account_for_task_list(&list_id)
                    .unwrap_or_else(|| LOCAL_ID.to_string());
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::Sections, &list_id)
                    .ok()
                    .flatten();
                let warm = has_snapshot(&state);
                let stale = is_stale(&state, SWR_TTL_SECS);
                // Cache-only serve — a never-warmed list serves EMPTY and the
                // spawned refresh fills the view via the cache-updated push
                // (see get_events_json; the old live read stalled the serial
                // native queue at first-paint time).
                let mut sections = self
                    .cache
                    .read_sections(&account, &list_id)
                    .unwrap_or_default();
                if !warm || stale {
                    let cache_bg = Arc::clone(&self.cache);
                    let ext_bg = Arc::clone(&ext);
                    let acc = account.clone();
                    let list = list_id.clone();
                    spawn_item_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::Sections,
                        account,
                        list_id.clone(),
                        move || async move {
                            refresh_sections(&cache_bg, ext_bg.as_ref(), &acc, &list).await
                        },
                    );
                }
                // External sections have no provider colour field — stamp any
                // host-local colour overrides. Runs on BOTH the cached and the
                // live path (the cache stores the raw provider section without
                // the override), exactly like the pre-cache live read did.
                // (Local sections carry their own synced binding, handled by the
                // None arm.)
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
                self.invalidate_sections_cache(&list_id);
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
                self.invalidate_sections_cache(&section.list_id);
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
                    .map_err(map_store_err)?;
                self.invalidate_sections_cache(&lid);
                Ok(())
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
                    &self.plugin_manager,
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

    /// The same question, asked with an adapter kind and the shared schema
    /// form's values — the mobile twin of the desktop
    /// `preview_sync_target_values`.
    ///
    /// The safe one to move first: it reaches the target and reports what is
    /// there, and commits nothing, so the two entry points cannot disagree
    /// about anything that outlives the call.
    ///
    /// Refuses with the host-key error for a target whose fingerprint this
    /// device has not confirmed. That is a step in the flow rather than a
    /// fault — the caller answers it with the fingerprint probe and the trust
    /// gesture, exactly as the account picker does.
    pub fn preview_sync_target_values_json(
        &self,
        request_json: String,
    ) -> Result<String, StoreError> {
        let req: SchemaFormRequest = from_json("sync target form", &request_json)?;
        let shared = self.db.shared();
        let adapter = host_core::sync_target::preview_adapter(
            &HostSyncPlugins(&self.plugin_manager),
            &UserPrefsHostKeyVerifier::new(shared),
            &req.adapter_kind,
            &req.values,
        )
        .map_err(connect_err)?;
        let preview = self
            .runtime
            .block_on(async { self.onboarding.preview(adapter.as_ref()).await })
            .map_err(sync_err)?;
        to_json(&preview)
    }

    /// Resume a device flagged STALE (§19.10): it fell so far behind the dataset
    /// that incremental sync can't safely catch up, so re-onboard from the
    /// configured target, then drop the latched stale flag (subsequent rounds run
    /// normally + the status clears). Mirrors the desktop `resume_stale_device`
    /// minus its Tauri status-emit — the mobile status hook polls. Returns the
    /// OnboardingReport JSON. Rejects when no sync target is configured.
    pub fn resume_stale_device_json(&self) -> Result<String, StoreError> {
        let adapter =
            self.orchestrator
                .adapter_handle()
                .ok_or_else(|| StoreError::Unsupported {
                    detail: "no sync adapter configured".to_string(),
                })?;
        let report = self
            .runtime
            .block_on(async { self.onboarding.resume_from_stale(adapter.as_ref()).await })
            .map_err(sync_err)?;
        self.orchestrator.clear_stale_device();
        // A stale-resume re-onboards from the target, so it can materialise
        // accounts this device never saw — same registration gap as a join
        // (and the snapshot path doesn't show up in `report.applied` either).
        self.register_synced_accounts();
        to_json(&report)
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
        // Joining just materialised this device's external accounts. Register
        // their adapters NOW — unconditionally, not gated on `report.applied`:
        // a dataset that was compacted into a snapshot restores every account
        // through the SNAPSHOT, which `applied` (a log-event counter) doesn't
        // see. Self-gating: a no-op when nothing was missing.
        self.register_synced_accounts();
        to_json(&report)
    }

    /// "Start fresh" (§19.11) — overwrite the target's `meta.json` so it names
    /// only THIS device, optionally minting end-to-end encryption from a
    /// passphrase, then activate + persist the target. The mobile twin of the
    /// desktop `adopt_local_dataset`; the unified Connect button uses it to
    /// INITIALISE an empty target (and, behind a confirm, to overwrite an
    /// existing one). Unlike `enable_sync_encryption_json` (which always enables
    /// E2E), a blank/whitespace passphrase means a PLAINTEXT fresh dataset, not
    /// an error — matching the desktop adopt semantics. Returns the
    /// OnboardingReport JSON.
    pub fn adopt_local_dataset_json(
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
        // Mint v2 E2E material only when a passphrase is given (a fresh DEK + a
        // KEK derived from the passphrase + fresh params; the wrapped DEK is
        // recorded in the params written to meta.json) — mirrors
        // enable_sync_encryption_json's fresh branch. A blank passphrase leaves
        // the fresh dataset plaintext (the user can enable E2E afterwards).
        let (key, e2e_params): (Option<[u8; KEY_LEN]>, Option<EncryptionParams>) = match passphrase
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(pp) => {
                let mut params = EncryptionParams::fresh();
                let kek = derive_key(pp, &params).map_err(sync_err)?;
                let dek = fresh_data_key();
                let wrapped = wrap_key(&kek, &dek).map_err(sync_err)?;
                params.wrapped_data_key = Some(wrapped);
                (Some(dek), Some(params))
            }
            None => (None, None),
        };
        let adapter = wrap_if_encrypted(plain, key);
        let trimmed = device_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // Crash-safest order (no concurrent scheduler on mobile): adopt_local
        // writes the fresh meta.json + registers this device FIRST; only after it
        // succeeds do we activate, persist, and store the key + flag — so a
        // mid-failure leaves neither key nor flag set.
        let report = self
            .runtime
            .block_on(async {
                self.onboarding
                    .adopt_local(adapter.as_ref(), trimmed, e2e_params)
                    .await
            })
            .map_err(sync_err)?;
        self.orchestrator.configure(adapter);
        self.persist_sync_config(&req)?;
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        if let Some(k) = key {
            store_e2e_key(self.secret_store.as_ref(), &k)?;
            prefs
                .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
                .map_err(storage_err)?;
        } else {
            // A plaintext fresh dataset clears any stale E2E flag.
            let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
        }
        to_json(&report)
    }

    /// Join an existing dataset reached through the shared schema form's own
    /// values — the mobile twin of the desktop `accept_remote_dataset_values`.
    ///
    /// Everything [`Self::accept_remote_dataset_json`] does, except for what it
    /// writes down at the end: this one commits through
    /// [`host_core::sync_target::connect`], so the target becomes an ACCOUNT ROW
    /// plus this device's pointer at it, rather than a set of `sync.adapter.*`
    /// preferences only this device can see. A phone onboarded here is
    /// afterwards indistinguishable from one that picked the account on the sync
    /// screen — which is the point of the unification: the two entry points must
    /// not leave the device in two different states.
    pub fn accept_remote_dataset_values_json(
        &self,
        request_json: String,
        device_name: Option<String>,
        passphrase: Option<String>,
    ) -> Result<String, StoreError> {
        let req: SchemaFormRequest = from_json("sync target form", &request_json)?;
        let shared = self.db.shared();
        let plugins = HostSyncPlugins(&self.plugin_manager);
        let plain = host_core::sync_target::preview_adapter(
            &plugins,
            &UserPrefsHostKeyVerifier::new(shared.clone()),
            &req.adapter_kind,
            &req.values,
        )
        .map_err(connect_err)?;
        self.runtime
            .block_on(async { plain.test_connection().await })
            .map_err(sync_err)?;
        // Peek at meta.json first: an encrypted dataset needs its key derived
        // from the passphrase + the dataset's own params BEFORE anything reads a
        // snapshot or a log, because the applier needs plaintext bytes.
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
            Some(resolve_data_key(pp, &params).map_err(sync_err)?)
        } else {
            None
        };
        let adapter = wrap_if_encrypted(plain, key);
        let prefs = UserPrefsRepo::new(&shared);
        // Set before applying, reverted on failure: the snapshot's credential
        // restore is gated on this flag, so an E2E dataset applied while it is
        // still false drops every account's password and re-asks for all of them.
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
        self.orchestrator.configure(adapter);
        // The account row, its secrets, this device's half of the fields and the
        // pointer — the same write the sync screen makes.
        host_core::sync_target::connect(
            &prefs,
            &AccountsRepo::new(&shared),
            self.secret_store.as_ref(),
            &plugins,
            &req.adapter_kind,
            &req.values,
        )
        .map_err(connect_err)?;
        if let Some(k) = key {
            store_e2e_key(self.secret_store.as_ref(), &k)?;
        } else {
            let _ = prefs.delete(host_core::credential_sync::PREF_E2E_ENABLED);
        }
        // Joining just materialised this device's external accounts; a compacted
        // dataset restores them through the SNAPSHOT, which `report.applied`
        // never sees. Self-gating, so calling it unconditionally is free.
        self.register_synced_accounts();
        to_json(&report)
    }

    /// Start a fresh dataset on a target reached through the shared schema
    /// form's values — the mobile twin of the desktop
    /// `adopt_local_dataset_values`.
    ///
    /// Same body as [`Self::adopt_local_dataset_json`], same committing step as
    /// [`Self::accept_remote_dataset_values_json`]: an account row plus a
    /// pointer, not a preference. A blank passphrase means a PLAINTEXT fresh
    /// dataset rather than an error — encryption can be turned on afterwards.
    pub fn adopt_local_dataset_values_json(
        &self,
        request_json: String,
        device_name: Option<String>,
        passphrase: Option<String>,
    ) -> Result<String, StoreError> {
        let req: SchemaFormRequest = from_json("sync target form", &request_json)?;
        let shared = self.db.shared();
        let plugins = HostSyncPlugins(&self.plugin_manager);
        let plain = host_core::sync_target::preview_adapter(
            &plugins,
            &UserPrefsHostKeyVerifier::new(shared.clone()),
            &req.adapter_kind,
            &req.values,
        )
        .map_err(connect_err)?;
        self.runtime
            .block_on(async { plain.test_connection().await })
            .map_err(sync_err)?;
        let (key, e2e_params): (Option<[u8; KEY_LEN]>, Option<EncryptionParams>) = match passphrase
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(pp) => {
                let mut params = EncryptionParams::fresh();
                let kek = derive_key(pp, &params).map_err(sync_err)?;
                let dek = fresh_data_key();
                let wrapped = wrap_key(&kek, &dek).map_err(sync_err)?;
                params.wrapped_data_key = Some(wrapped);
                (Some(dek), Some(params))
            }
            None => (None, None),
        };
        let adapter = wrap_if_encrypted(plain, key);
        let trimmed = device_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // adopt_local writes the fresh meta.json + registers this device FIRST;
        // only then do we activate, commit and store the key, so a mid-failure
        // leaves neither key nor flag set.
        let report = self
            .runtime
            .block_on(async {
                self.onboarding
                    .adopt_local(adapter.as_ref(), trimmed, e2e_params)
                    .await
            })
            .map_err(sync_err)?;
        self.orchestrator.configure(adapter);
        let prefs = UserPrefsRepo::new(&shared);
        host_core::sync_target::connect(
            &prefs,
            &AccountsRepo::new(&shared),
            self.secret_store.as_ref(),
            &plugins,
            &req.adapter_kind,
            &req.values,
        )
        .map_err(connect_err)?;
        if let Some(k) = key {
            store_e2e_key(self.secret_store.as_ref(), &k)?;
            prefs
                .set(host_core::credential_sync::PREF_E2E_ENABLED, "true")
                .map_err(storage_err)?;
        } else {
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
            &self.plugin_manager,
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
        // Drive is deliberately absent. It reaches Aperio as a Google ACCOUNT
        // now, so its sign-in runs the account OAuth path and its refresh token
        // lands in that account's keychain slot — not in a pseudo-account
        // divorced from every row. The `sync.adapter.googledrive` slot is still
        // READ, by the legacy pre-migration build path, and never written here.
        let secret_account = match plugin_id.as_str() {
            PLUGIN_ID_DROPBOX => DROPBOX_SECRET_ACCOUNT,
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
        Ok(classify_host_key(&self.db.shared(), host_port, probe.fingerprint)?.to_string())
    }

    /// The §19.5 trust gesture for an ACCOUNT the user is about to sync
    /// through — the mobile twin of the desktop `preview_sync_account_host_key`.
    ///
    /// [`Self::select_sync_account`] refuses an account whose protocol pins host
    /// keys until this device has confirmed the server's fingerprint, which is
    /// the ordinary state of an SFTP account added under Settings → Accounts:
    /// that path never probes. Without this the refusal is a dead end, because
    /// the only other way to pin a fingerprint is the connect form and the sync
    /// screen no longer shows one.
    ///
    /// Answers `null` — no error — for an account whose adapter declares no
    /// `host_key_pin`. The desktop refuses that with an `invalid_input` CODE the
    /// panel branches on; this boundary has no code channel (a `StoreError`
    /// crosses as a message), so "this account has no fingerprint to check" is a
    /// VALUE here. It is also what lets the screen ask the question after any
    /// refusal without a network round-trip for the adapters that cannot
    /// produce this one.
    ///
    /// Nothing here names a protocol: WHICH fields hold the host and the port
    /// come from the schema's own `host_key_pin` declaration — the same one
    /// [`host_core::sync_target::from_account`] refuses on.
    pub fn preview_sync_account_host_key_json(
        &self,
        account_id: String,
    ) -> Result<String, StoreError> {
        let account_id = account_id.trim().to_string();
        let shared = self.db.shared();
        let account = AccountsRepo::new(&shared)
            .get(&account_id)
            .map_err(storage_err)?
            .ok_or(StoreError::NotFound)?;
        let Some((plugin_id, schema)) = host_core::sync_target::SyncPlugins::resolve(
            &HostSyncPlugins(&self.plugin_manager),
            account.adapter_kind.as_str(),
        ) else {
            return Err(StoreError::Storage {
                detail: format!("no loaded plugin serves `{}`", account.adapter_kind),
            });
        };
        // No pin declared → nothing to confirm. `null`, not an error: see above.
        let Some(pin) = schema.host_key_pin else {
            return Ok("null".to_string());
        };
        // WHICH server, resolved the one way both hosts resolve it — the row
        // plus this device's half, port as text either way.
        let Some(info) = host_core::sync_target::account_host_key_pin(
            &account,
            &UserPrefsRepo::new(&shared),
            &UserPrefsHostKeyVerifier::new(shared.clone()),
            &HostSyncPlugins(&self.plugin_manager),
        ) else {
            return Ok("null".to_string());
        };
        let host = info.host.clone();
        let port = info.port.clone();
        let parsed_port: u16 = port.parse().unwrap_or_default();
        if host.is_empty() || parsed_port == 0 {
            return Err(StoreError::InvalidField {
                field: pin.host_field.clone(),
                detail: "this account does not say which server to probe".to_string(),
            });
        }
        // Under the plugin's own field names, and built by hand rather than
        // through `json!` because the keys are the declaration's, not literals.
        let mut args = serde_json::Map::new();
        args.insert(
            pin.host_field.clone(),
            serde_json::Value::String(host.clone()),
        );
        args.insert(pin.port_field.clone(), serde_json::Value::from(parsed_port));
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .probe_host_key(&plugin_id, &serde_json::Value::Object(args).to_string())
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
        // The same key `merge_pin` looks the pin up under, or a fingerprint the
        // user confirms here stays invisible to the build that needs it.
        Ok(classify_host_key(&shared, format!("{host}:{port}"), probe.fingerprint)?.to_string())
    }

    /// What this device has confirmed about an account's server — no network.
    ///
    /// The counterpart to [`Self::preview_sync_account_host_key_json`], which
    /// dials the server to see what it is presenting NOW. This one reports the
    /// decision the user already made, so the sync screen can show it and offer
    /// to revoke it while the server is unreachable — which is exactly when
    /// revoking matters.
    ///
    /// `null` for an account that is gone or whose adapter declares no
    /// `host_key_pin`; a `host_port` of `null` INSIDE a value means the row does
    /// not say which server, which is a different thing with a different repair.
    pub fn sync_account_host_key_pin_json(&self, account_id: String) -> Result<String, StoreError> {
        let account_id = account_id.trim().to_string();
        let shared = self.db.shared();
        let Some(account) = AccountsRepo::new(&shared)
            .get(&account_id)
            .map_err(storage_err)?
        else {
            return Ok("null".to_string());
        };
        match host_core::sync_target::account_host_key_pin(
            &account,
            &UserPrefsRepo::new(&shared),
            &UserPrefsHostKeyVerifier::new(shared.clone()),
            &HostSyncPlugins(&self.plugin_manager),
        ) {
            Some(info) => to_json(&info),
            None => Ok("null".to_string()),
        }
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

    /// What this device calls itself in every other device's list, as JSON:
    /// `{"configured": string|null, "suggested": string|null}`.
    ///
    /// `suggested` is always `null` here. The desktop host reads the machine's
    /// host name for it; on a phone the equivalent answer belongs to the OS
    /// layer above this one (`expo-constants` knows the device name and the
    /// Rust core deliberately does not), so the mobile UI fills the suggestion
    /// in itself and this stays honest about knowing nothing.
    pub fn sync_device_name_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let prefs = host_core::user_prefs::UserPrefsRepo::new(&shared);
        to_json(&serde_json::json!({
            "configured": host_core::device_names::local_device_name(&prefs),
            "suggested": Option::<String>::None,
        }))
    }

    /// Rename this device; a blank name clears it.
    ///
    /// Nothing is pushed here. The heartbeat compares the stored name against
    /// the one in this device's `meta.json` record and pushes when they differ,
    /// so the rename reaches the other devices on the next round — the only
    /// ordering that cannot leave the two disagreeing.
    pub fn set_sync_device_name(&self, name: String) -> Result<(), StoreError> {
        let shared = self.db.shared();
        let prefs = host_core::user_prefs::UserPrefsRepo::new(&shared);
        host_core::device_names::set_local_device_name(&prefs, &name).map_err(|err| {
            StoreError::Storage {
                detail: format!("save device name: {err}"),
            }
        })
    }

    /// Every device registered on the dataset this one syncs through, as a JSON
    /// array of `DeviceSummary`. A live read of `meta.json`, so it needs a
    /// configured target.
    pub fn list_sync_devices_json(&self) -> Result<String, StoreError> {
        let adapter =
            self.orchestrator
                .adapter_handle()
                .ok_or_else(|| StoreError::Unsupported {
                    detail: "no sync adapter configured".to_string(),
                })?;
        let devices = self
            .runtime
            .block_on(async { self.onboarding.list_devices(adapter.as_ref()).await })
            .map_err(sync_err)?;
        to_json(&devices)
    }

    /// Drop a device's registry entry.
    ///
    /// Not a revocation and not a delete: it removes the claim that the device
    /// is still participating, which is what frees the compactor to collect
    /// logs nobody will read. The log FILES stay, and a device that still runs
    /// re-registers on its next round. Refuses this device's own id — the next
    /// heartbeat would undo it.
    pub fn forget_sync_device(&self, device_id: String) -> Result<(), StoreError> {
        let device_id = device_id.trim().to_string();
        if device_id.is_empty() {
            return Err(StoreError::InvalidField {
                field: "device_id".to_string(),
                detail: "device_id must not be empty".to_string(),
            });
        }
        let adapter =
            self.orchestrator
                .adapter_handle()
                .ok_or_else(|| StoreError::Unsupported {
                    detail: "no sync adapter configured".to_string(),
                })?;
        self.runtime
            .block_on(async {
                self.onboarding
                    .forget_device(adapter.as_ref(), &device_id)
                    .await
            })
            .map_err(sync_err)
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
        // A different backend invalidates every remote-missing sound verdict
        // (mirrors the orchestrator clearing its per-session pushed lengths).
        host_core::sound_assets::reset_missing_cache();
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

    /// Point this device at an account it ALREADY has, and sync through it —
    /// the mobile twin of the desktop `select_sync_account`.
    ///
    /// The sync screen's whole question, in one call. The account was added
    /// under Settings → Accounts, or arrived with a restored dataset; nothing
    /// here takes a form, a host or a password, because none of that is being
    /// decided — the row already holds it.
    ///
    /// [`host_core::sync_target::from_account`] opens the row through the
    /// plugin's own schema, so the ways it can refuse are the ways the user can
    /// fix, and each fix is different: an unconfirmed host key (§19.5) is
    /// repaired with [`Self::preview_sync_account_host_key_json`] then
    /// [`Self::trust_sftp_host_key`]; a credential that is not in this device's
    /// keychain is repaired on the accounts screen; a kind no loaded plugin
    /// serves is asked about BEFORE the builder, because `PluginRefused` also
    /// covers a plugin that IS installed and disliked the config, and "install
    /// the plugin" is the wrong instruction for that.
    ///
    /// Nothing is written down until the target has been probed AND the §19.13
    /// compatibility and E2E gates have passed ([`Self::wrap_for_target`]), so a
    /// refusal leaves this device syncing exactly where it did before. Same
    /// ordering as [`Self::configure_sync_adapter_json`], and for the same
    /// reason: a rejected target must not be what the next launch comes up on.
    pub fn select_sync_account(&self, account_id: String) -> Result<(), StoreError> {
        let account_id = account_id.trim().to_string();
        if account_id.is_empty() {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: "no account id supplied".to_string(),
            });
        }
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let account = AccountsRepo::new(&shared)
            .get(&account_id)
            .map_err(storage_err)?
            .ok_or(StoreError::NotFound)?;
        let plugins = HostSyncPlugins(&self.plugin_manager);
        if host_core::sync_target::SyncPlugins::resolve(&plugins, account.adapter_kind.as_str())
            .is_none()
        {
            return Err(StoreError::Unsupported {
                detail: format!("no loaded plugin serves `{}`", account.adapter_kind),
            });
        }
        // A different backend invalidates every remote-missing sound verdict
        // (mirrors the orchestrator clearing its per-session pushed lengths).
        host_core::sound_assets::reset_missing_cache();
        let adapter = host_core::sync_target::from_account(
            &account,
            &prefs,
            self.secret_store.as_ref(),
            &UserPrefsHostKeyVerifier::new(shared.clone()),
            &plugins,
        )
        .map_err(unbuildable_err)?;
        // Probe before keeping it active so a bad address / credential fails
        // here rather than on the first silent sync round.
        self.runtime
            .block_on(async { adapter.test_connection().await })
            .map_err(sync_err)?;
        let adapter = self.wrap_for_target(adapter)?;
        self.orchestrator.configure(adapter);
        // The one thing this call writes: which account this device syncs
        // through. The row itself is the user's, added elsewhere and untouched
        // here — moving off it must not disturb it, which is exactly what
        // separates this from the connect path.
        host_core::sync_target::select_account(&prefs, Some(&account.id)).map_err(storage_err)
    }

    /// Disconnect the configured sync target: deconfigure the orchestrator and
    /// remove everything a restore path could act on — the account row, its
    /// credentials and device-local half, the pointer, and the legacy
    /// preferences and keychain pseudo-accounts. The dataset's encryption key
    /// stays; it is not a property of the target.
    ///
    /// Keeping the fields "so reconnecting is one tap" is what this used to do,
    /// and it is why a disconnected phone came back up on the next launch
    /// uploading to the target it had been told to stop using. Reconnecting now
    /// means re-entering the target. Mirrors the desktop
    /// `configure_sync_adapter({kind:"none"})`.
    pub fn disconnect_sync(&self) -> Result<(), StoreError> {
        self.orchestrator.deconfigure();
        let shared = self.db.shared();
        host_core::sync_target::disconnect(
            &UserPrefsRepo::new(&shared),
            &AccountsRepo::new(&shared),
            self.secret_store.as_ref(),
        )
        .map_err(storage_err)
    }

    /// Non-secret summary of the configured sync target as JSON — `null` when
    /// nothing is configured, else `{"kind","detail"}`. `detail` is a human
    /// "user@url" / "host:port/path" string built from the stored field prefs,
    /// never a secret. Mirrors the desktop `get_sync_adapter_summary`.
    pub fn get_sync_adapter_summary_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        // The account this device syncs through answers first, and answers
        // alone: once the pointer is set the preferences below are a record
        // nothing maintains, and a card built from them would name the target
        // the user moved off.
        match host_core::sync_target::summary(&prefs, &AccountsRepo::new(&shared)) {
            host_core::sync_target::SummaryOutcome::Chosen(kind, detail) => {
                return to_json(&Some(SyncAdapterSummary {
                    kind,
                    detail,
                    account_id: host_core::sync_target::selected_account_id(&prefs),
                }));
            }
            // See the desktop twin: a pointer to a missing row must not fall
            // through to preferences that still describe the old target.
            host_core::sync_target::SummaryOutcome::Missing => {
                return to_json(&Option::<SyncAdapterSummary>::None);
            }
            host_core::sync_target::SummaryOutcome::NotChosen => {}
        }
        let kind = prefs
            .get(PREF_ADAPTER_KIND)
            .map_err(storage_err)?
            .filter(|stored| !is_unconfigured(Some(stored)));
        let Some(kind) = kind else {
            return to_json(&Option::<SyncAdapterSummary>::None);
        };
        let detail = match kind.as_str() {
            "local" => prefs
                .get(PREF_LOCAL_PATH)
                .map_err(storage_err)?
                .unwrap_or_default(),
            "webdav" => {
                let url = prefs
                    .get(PREF_WEBDAV_URL)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                let user = prefs
                    .get(PREF_WEBDAV_USER)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                if user.is_empty() {
                    url
                } else {
                    format!("{user}@{url}")
                }
            }
            "sftp" => {
                let host = prefs
                    .get(PREF_SFTP_HOST)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                let port = prefs
                    .get(PREF_SFTP_PORT)
                    .map_err(storage_err)?
                    .unwrap_or_else(|| "22".into());
                let user = prefs
                    .get(PREF_SFTP_USER)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                let path = prefs
                    .get(PREF_SFTP_PATH)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                format!("{user}@{host}:{port}{path}")
            }
            "ftp" => {
                let host = prefs
                    .get(PREF_FTP_HOST)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                let port = prefs
                    .get(PREF_FTP_PORT)
                    .map_err(storage_err)?
                    .unwrap_or_else(|| "21".into());
                let user = prefs
                    .get(PREF_FTP_USER)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                let path = prefs
                    .get(PREF_FTP_PATH)
                    .map_err(storage_err)?
                    .unwrap_or_default();
                format!("{user}@{host}:{port}{path}")
            }
            "dropbox" => prefs
                .get(PREF_DROPBOX_PATH)
                .map_err(storage_err)?
                .unwrap_or_default(),
            "googledrive" => prefs
                .get(PREF_GOOGLEDRIVE_FOLDER_NAME)
                .map_err(storage_err)?
                .unwrap_or_default(),
            "none" => return to_json(&Option::<SyncAdapterSummary>::None),
            _ => String::new(),
        };
        // The legacy reader answered, so there is no row to name.
        to_json(&Some(SyncAdapterSummary {
            kind,
            detail,
            account_id: None,
        }))
    }

    /// Run one sync round (push local pending logs, fetch + apply foreign ones,
    /// compaction audit) and return the `SyncRoundReport` as JSON. Errors with
    /// "not configured" until `configure_sync_adapter_json` has run. Records the
    /// round's outcome in the failure latch (success resets it) AND appends a
    /// `sync_log` row (the desktop scheduler's job; mobile has no scheduler, so
    /// the round records itself here). `trigger` is the wire SyncTrigger string
    /// (`"manual"` for the Settings button, `"app_start"`/`"periodic"` for the
    /// launch/foreground rounds — unknown ⇒ `manual`).
    pub fn sync_now_json(&self, trigger: String) -> Result<String, StoreError> {
        let started = std::time::Instant::now();
        let result = self
            .runtime
            .block_on(async { self.orchestrator.sync_now().await });
        let duration_ms = Some(started.elapsed().as_millis() as u64);
        let trig = parse_sync_trigger(&trigger);
        match result {
            Ok(report) => {
                self.progress.record_success();
                self.record_sync_round(
                    trig,
                    true,
                    &SyncLogCounters {
                        pushed_logs: Some(report.pushed_logs as u32),
                        fetched_logs: Some(report.fetched_logs as u32),
                        applied: Some(report.applied as u32),
                        conflicts: Some(report.conflicts as u32),
                    },
                    duration_ms,
                    None,
                );
                // The round may have created external ACCOUNTS (an account
                // added on another device). The applier only writes rows —
                // bringing their adapters up is the host's job, or they stay
                // dead until the next app start.
                //
                // Deliberately UNconditional: `report.applied` counts only
                // applied LOG events, while the §19.10 inline auto-resume
                // restores accounts from a SNAPSHOT (its report is discarded
                // before the round's is built), so `applied` can be 0 with
                // brand-new accounts on disk. The call is self-gating —
                // one indexed query, immediate return when nothing is new.
                self.register_synced_accounts();
                to_json(&report)
            }
            Err(e) => {
                self.progress.record_failure(e.code());
                let err = sync_err(e);
                self.record_sync_round(
                    trig,
                    false,
                    &SyncLogCounters::default(),
                    duration_ms,
                    Some(&err.to_string()),
                );
                Err(err)
            }
        }
    }

    /// Manually trigger a compaction round (§19.10): snapshot the local state,
    /// push `snapshot.json`, advance `meta.json`'s snapshot timestamp, and GC
    /// every log file older than the new horizon. Mirrors the desktop
    /// `compact_now`; the scheduler runs this same path automatically at the
    /// §19.10 thresholds, so this is the "user got impatient" override. Rejects
    /// when no sync adapter is configured. Records the outcome in the `sync_log`
    /// (success or partial-failure) like the desktop's `record_compaction_outcome`
    /// — no push/fetch counts on a compaction, so `applied` carries the
    /// deleted-log count and the Protokoll renders "N old logs removed".
    pub fn compact_now_json(&self) -> Result<String, StoreError> {
        let Some(adapter) = self.orchestrator.adapter_handle() else {
            return Err(StoreError::Unsupported {
                detail: "no sync adapter configured".into(),
            });
        };
        let started = std::time::Instant::now();
        let result = self.runtime.block_on(async {
            self.orchestrator
                .compactor()
                .compact_now(adapter.as_ref())
                .await
        });
        let duration_ms = Some(started.elapsed().as_millis() as u64);
        match result {
            Ok(report) => {
                // A compaction succeeds even with some failed deletes; flag the
                // partial failure so the Protokoll shows it, mirroring desktop.
                let success = report.failed_deletes == 0;
                let error = (!success).then(|| {
                    format!(
                        "{} of {} log deletions failed",
                        report.failed_deletes,
                        report.deleted_logs + report.failed_deletes,
                    )
                });
                self.record_sync_round(
                    SyncTrigger::Compaction,
                    success,
                    &SyncLogCounters {
                        pushed_logs: None,
                        fetched_logs: None,
                        applied: Some(u32::try_from(report.deleted_logs).unwrap_or(u32::MAX)),
                        conflicts: None,
                    },
                    duration_ms,
                    error.as_deref(),
                );
                to_json(&report)
            }
            Err(e) => {
                let err = sync_err(e);
                self.record_sync_round(
                    SyncTrigger::Compaction,
                    false,
                    &SyncLogCounters::default(),
                    duration_ms,
                    Some(&err.to_string()),
                );
                Err(err)
            }
        }
    }

    /// Push the local pending logs without fetching (call from RN AppState
    /// "background"). Returns the number of logs pushed. Records the outcome in
    /// the failure latch AND a `sync_log` row, like `sync_now`. `trigger` is the
    /// wire SyncTrigger string (`"kick"` for the debounced push, `"app_exit"`
    /// for the background flush — unknown ⇒ `kick`).
    pub fn push_now(&self, trigger: String) -> Result<u32, StoreError> {
        let started = std::time::Instant::now();
        let result = self
            .runtime
            .block_on(async { self.orchestrator.push_now().await });
        let duration_ms = Some(started.elapsed().as_millis() as u64);
        let trig = parse_push_trigger(&trigger);
        match result {
            Ok(pushed) => {
                self.progress.record_success();
                self.record_sync_round(
                    trig,
                    true,
                    &SyncLogCounters {
                        pushed_logs: Some(pushed as u32),
                        ..SyncLogCounters::default()
                    },
                    duration_ms,
                    None,
                );
                Ok(pushed as u32)
            }
            Err(e) => {
                self.progress.record_failure(e.code());
                let err = sync_err(e);
                self.record_sync_round(
                    trig,
                    false,
                    &SyncLogCounters::default(),
                    duration_ms,
                    Some(&err.to_string()),
                );
                Err(err)
            }
        }
    }

    /// Recent `sync_log` rows as a JSON `SyncLogEntry[]` (newest first), capped
    /// at `limit` (and the table's own retention cap). The mobile Protokoll
    /// viewer. Mirrors the desktop `list_sync_log_entries`.
    pub fn list_sync_log_json(&self, limit: u32) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = SyncLogRepo::new(&shared);
        let entries = repo.list(limit).map_err(|e| StoreError::Storage {
            detail: e.to_string(),
        })?;
        to_json(&entries)
    }

    /// Drop every `sync_log` row (the "clear history" action — also useful
    /// before sharing a screen). Mirrors the desktop `clear_sync_log`.
    pub fn clear_sync_log(&self) -> Result<(), StoreError> {
        let shared = self.db.shared();
        let repo = SyncLogRepo::new(&shared);
        repo.clear().map_err(|e| StoreError::Storage {
            detail: e.to_string(),
        })
    }

    // ─── External cache (SWR) control ──────────────────────────────────────────
    //
    // The read paths above serve external containers stale-while-revalidate +
    // self-warm on a cold/stale read; these expose the desktop's explicit cache
    // controls: a JS observer (so a finished background refresh re-renders the
    // open view live), a manual "refresh now", a "last updated" status, and an
    // on-foreground warm (the mobile stand-in for the desktop periodic loop).

    /// Register the JS-side cache observer. A finished background refresh / warm
    /// pass then pushes `cache_updated` (the RN layer re-reads the matching view)
    /// and `refresh_status` across the bridge. Until this is called the pushes
    /// are dropped — the cache still populates; the UI just re-reads on its own
    /// focus / periodic-sync until the live push is wired.
    pub fn set_cache_observer(&self, observer: Arc<dyn CacheObserverBridge>) {
        self.cache_observer.set(observer);
    }

    /// Kick an immediate warm pass over every external account's containers +
    /// in-window events (the manual "refresh now"). Fire-and-forget on the Host
    /// worker thread; per-container `cache_updated` + the `refresh_status`
    /// transitions arrive via the observer. Mirrors the desktop
    /// `refresh_external_cache`.
    pub fn refresh_external_cache(&self) {
        let refresher = Arc::clone(&self.cache_refresher);
        self.runtime.handle().spawn(async move {
            // The genuine manual "refresh now" is an explicit user action —
            // surface any failure at once (forced; see warm_all). The
            // automatic on-foreground warm uses warm_cache_on_foreground,
            // which runs UN-forced.
            refresher.warm_all(true).await;
        });
    }

    /// Write one line into the app's own rolling log — the file the Logs
    /// screen shows and the support bundle exports.
    ///
    /// The JS layer had no way in, and the gap was only visible from the
    /// outside: the background round is the one code path a user cannot watch
    /// happen, and its two most interesting steps — did the provider warm pass
    /// finish, was the widget snapshot rewritten — left no trace anywhere. The
    /// sync log records the peer round and nothing else, so "the pull worked
    /// but the widget was wrong" had no evidence on either side of it.
    ///
    /// Level names match the filter the user picks; anything unrecognised is
    /// logged at info rather than dropped, because losing a diagnostic to a
    /// typo in the diagnostic is the worst possible trade.
    pub fn log_line(&self, level: String, message: String) {
        match level.as_str() {
            "error" => tracing::error!(target: "aperio::app", "{message}"),
            "warn" => tracing::warn!(target: "aperio::app", "{message}"),
            "debug" => tracing::debug!(target: "aperio::app", "{message}"),
            "trace" => tracing::trace!(target: "aperio::app", "{message}"),
            _ => tracing::info!(target: "aperio::app", "{message}"),
        }
    }

    /// The warm-pass status (`{refreshing, last_refreshed_at}`) as JSON — the
    /// "last updated" / spinner surface. Mirrors `get_cache_refresh_status`.
    pub fn get_cache_refresh_status_json(&self) -> Result<String, StoreError> {
        to_json(&self.cache_refresher.status())
    }

    /// Every account's currently-failing containers as a JSON
    /// `AccountRefreshErrors[]` — the per-account error surface, so a
    /// revoked/wrong provider password shows up as a warning instead of
    /// silent staleness. Mirrors the desktop `get_refresh_errors`.
    pub fn refresh_errors_json(&self) -> Result<String, StoreError> {
        let errors = self
            .cache
            .refresh_errors()
            .map_err(|e| StoreError::Storage {
                detail: e.to_string(),
            })?;
        to_json(&errors)
    }

    /// Warm the cache when the app foregrounds — the mobile stand-in for a tick
    /// of the desktop periodic warm loop (which mobile can't run while
    /// backgrounded). This is AUTOMATIC (not a user action), so it runs the
    /// pass UN-forced, exactly like the desktop app-start/periodic pass: a
    /// cold-start/resume network blip must be confirmed by a second attempt
    /// before it surfaces, instead of alarming on the first failure. Only the
    /// genuine manual `refresh_external_cache` is forced.
    pub fn warm_cache_on_foreground(&self) {
        let refresher = Arc::clone(&self.cache_refresher);
        self.runtime.handle().spawn(async move {
            refresher.warm_all(false).await;
        });
    }

    /// Force a FULL cold re-sync of one external account: clear its delta tokens
    /// + cached window across every container, then kick a warm pass so each
    /// re-bootstraps from the provider. Cached rows stay as an offline fallback
    /// until replaced; credentials are untouched (no re-auth). The recovery
    /// action for a "stuck" external cache — a bootstrap that enumerated an
    /// incomplete resource set yet persisted a sync-token, so later deltas
    /// reported "no changes" over permanently-missing events. Mirrors the desktop
    /// `reset_account_sync` command.
    pub fn reset_account_sync(&self, account_id: String) -> Result<(), StoreError> {
        // Refuse rather than no-op — the desktop twin's reasoning verbatim: a
        // warm pass only visits registered adapters, so this would otherwise
        // wipe nothing, fetch nothing and report success.
        if !self.registry.has_adapter(&account_id) {
            return Err(StoreError::Unsupported {
                detail: "this account is not signed in on this device, so there                          is nothing to re-sync — reconnect it under Settings,                          Accounts"
                    .to_string(),
            });
        }
        self.cache
            .reset_account_sync(&account_id)
            .map_err(|e| StoreError::Storage {
                detail: e.to_string(),
            })?;
        self.refresh_external_cache();
        Ok(())
    }

    // ─── Device calendar + reminders (iOS EventKit / Android CalendarProvider) ───
    //
    // Mobile-only, host-internal: the adapter wraps a native bridge the OS module
    // installs after open. Its account is DEVICE-LOCAL — create/delete/rename do
    // NOT emit `account.*` sync events (see `create_account_json`), so it never
    // appears on the user's other devices, and the cross-device applier never
    // sees a kind it can't construct.

    /// Install the native device calendar/reminder bridge (iOS today; Android
    /// has none yet). Stores it and registers any already-persisted
    /// device-calendar account so it's routable without an app restart (bootstrap
    /// at `open` skipped it — no bridge yet). The native module calls this once,
    /// right after [`Host::open`].
    pub fn set_device_event_store(&self, bridge: Arc<dyn DeviceEventStoreBridge>) {
        let provider: Arc<dyn DeviceCalendarProvider> = Arc::new(BridgeDeviceProvider { bridge });
        if let Ok(mut guard) = self.device_provider.write() {
            *guard = Some(Arc::clone(&provider));
        }
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        if let Ok(accounts) = repo.list() {
            for account in accounts {
                if account.adapter_kind == host_core::builtin_adapters::device_calendar_kind() {
                    self.register_device_adapter(&account.id, Arc::clone(&provider));
                }
            }
        }
    }

    /// Run the OS permission prompt for the device calendar / reminders. Drives
    /// the add-account "grant access" step: the UI calls this, and on `true`
    /// proceeds to `create_account` for the `device_calendar` kind. An
    /// `InvalidField` means no native bridge is installed (e.g. Android).
    pub fn request_device_calendar_access(
        &self,
        events: bool,
        reminders: bool,
    ) -> Result<bool, StoreError> {
        let provider = self
            .device_provider()
            .ok_or_else(|| StoreError::InvalidField {
                field: "adapter_kind".to_string(),
                detail: "device calendar is not available on this platform".to_string(),
            })?;
        provider
            .request_access(events, reminders)
            .map_err(|e| StoreError::Storage {
                detail: e.to_string(),
            })
    }

    // ─── Contact sync (§10.5) ────────────────────────────────────────────────────
    //
    // The contact-sync core lives in host-core; mobile drives `run_sync` from the
    // manual button / foreground (the desktop wraps it in a tokio loop). The
    // observer delivers the "a pass finished" broadcast to JS; the interval +
    // include-read-only prefs are written DEVICE-LOCAL (the desktop commands do
    // too — they're a per-device cadence, not synced settings).

    /// Register the JS-side contact-sync observer. A finished pass then pushes
    /// `contacts_synced` (the RN layer updates the footer + re-reads contacts)
    /// across the bridge. Until this is called the pushes are dropped — passes
    /// still run; the UI just re-reads on its own.
    pub fn set_contact_sync_observer(&self, observer: Arc<dyn ContactSyncObserverBridge>) {
        self.contact_sync_observer.set(observer);
    }

    /// Run one contact-sync pass now — the mobile stand-in for the desktop tokio
    /// loop, driven from a foreground trigger / the manual "Sync now" button.
    /// `include_read_only`: `None` reads the persisted pref (matches the desktop
    /// manual button); `Some(_)` overrides it. Returns `false` when a pass was
    /// already in flight (the core dedupes).
    pub fn sync_contacts_now(&self, include_read_only: Option<bool>) -> Result<bool, StoreError> {
        let effective =
            include_read_only.unwrap_or_else(|| self.contact_sync.read_include_read_only_on_sync());
        let observer = Arc::clone(&self.contact_sync_observer);
        Ok(self
            .runtime
            .block_on(async { self.contact_sync.run_sync(&*observer, effective).await }))
    }

    /// The contact-sync status (`{last_synced_at, interval_minutes, in_flight,
    /// include_read_only_on_sync}`) as JSON — the footer + Settings seed.
    pub fn get_contacts_sync_status_json(&self) -> Result<String, StoreError> {
        to_json(&self.contact_sync.status())
    }

    /// Persist the periodic-sync interval (minutes), clamped to [1, 1440] like
    /// the desktop command. Device-local; returns the clamped value the UI
    /// echoes back.
    pub fn set_contacts_sync_interval(&self, minutes: u32) -> Result<u32, StoreError> {
        let clamped = minutes.clamp(1, 24 * 60);
        let shared = self.db.shared();
        UserPrefsRepo::new(&shared)
            .set(
                host_core::contact_sync::PREF_SYNC_INTERVAL_MINUTES,
                &clamped.to_string(),
            )
            .map_err(storage_err)?;
        Ok(clamped)
    }

    /// Persist the "also pull read-only directories" toggle (the literal
    /// `"true"`/`"false"`, matching the desktop command). Device-local.
    pub fn set_contacts_include_read_only_on_sync(&self, enabled: bool) -> Result<(), StoreError> {
        let shared = self.db.shared();
        UserPrefsRepo::new(&shared)
            .set(
                host_core::contact_sync::PREF_INCLUDE_READ_ONLY_ON_SYNC,
                if enabled { "true" } else { "false" },
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Drop every external adapter's in-memory contact cache + reset
    /// `contacts.lastSyncedAt` to "never" (the "Cache leeren" action). Returns
    /// the number of accounts the invalidate succeeded against — partial success
    /// is the right outcome when one account's server is unreachable. Per-adapter
    /// errors are swallowed (cal-ffi has no logger), mirroring the desktop body
    /// minus the warn. Distinct from the per-list SWR `invalidate_contacts_cache`.
    pub fn clear_contacts_cache(&self) -> Result<u32, StoreError> {
        let succeeded = self.runtime.block_on(async {
            let mut n = 0u32;
            for (_account, adapter) in self.registry.snapshot_contact_adapters() {
                if adapter.invalidate_contacts_cache().await.is_ok() {
                    n += 1;
                }
            }
            n
        });
        let shared = self.db.shared();
        let _ = UserPrefsRepo::new(&shared).delete(host_core::contact_sync::PREF_LAST_SYNCED_AT);
        Ok(succeeded)
    }

    // ─── Diagnostics / logs (§ Diagnostics) ──────────────────────────────────────
    //
    // The rolling-file log sink is brought up in Host::open (process-global); the
    // level is a DEVICE-LOCAL pref (logging.level), the live filter is reloaded
    // through the cached handle. These mirror the desktop commands/logs.rs 1:1.

    /// The persisted log level, or the default when unset.
    pub fn get_log_level(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        Ok(UserPrefsRepo::new(&shared)
            .get(host_core::logging::PREF_LOG_LEVEL)
            .map_err(storage_err)?
            .unwrap_or_else(|| host_core::logging::DEFAULT_LEVEL.to_string()))
    }

    /// Change the live verbosity + persist the choice (device-local). Validated
    /// against the known level set so a bad value can't be stored or silence
    /// logging.
    pub fn set_log_level(&self, level: String) -> Result<(), StoreError> {
        if !matches!(
            level.as_str(),
            "error" | "warn" | "info" | "debug" | "trace"
        ) {
            return Err(StoreError::InvalidField {
                field: "level".into(),
                detail: format!("unknown log level '{level}'"),
            });
        }
        crate::logging::set_level(&level);
        let shared = self.db.shared();
        UserPrefsRepo::new(&shared)
            .set(host_core::logging::PREF_LOG_LEVEL, &level)
            .map_err(storage_err)?;
        Ok(())
    }

    /// Tail of the newest log file for the in-app viewer (default 500 lines).
    pub fn get_recent_logs(&self, lines: Option<u32>) -> Result<String, StoreError> {
        let dir = crate::logging::logs_dir().ok_or_else(|| StoreError::Storage {
            detail: "logging not initialised".into(),
        })?;
        Ok(host_core::logging::recent_lines(
            dir,
            lines.unwrap_or(500) as usize,
        ))
    }

    /// The full (optionally redacted, default true) log bundle as a string — for
    /// the Share sheet. Capped to the most-recent ~2 MB so a huge trace bundle
    /// doesn't choke the share channel.
    pub fn collect_logs(&self, redact: Option<bool>) -> Result<String, StoreError> {
        let dir = crate::logging::logs_dir().ok_or_else(|| StoreError::Storage {
            detail: "logging not initialised".into(),
        })?;
        Ok(host_core::logging::collect(
            dir,
            redact.unwrap_or(true),
            Some(2 * 1024 * 1024),
        ))
    }

    /// Remove the rotated log files (the active one is kept).
    pub fn clear_logs(&self) -> Result<(), StoreError> {
        let dir = crate::logging::logs_dir().ok_or_else(|| StoreError::Storage {
            detail: "logging not initialised".into(),
        })?;
        host_core::logging::clear(dir);
        Ok(())
    }

    /// The on-disk logs directory, for display.
    pub fn logs_dir_path(&self) -> Result<String, StoreError> {
        let dir = crate::logging::logs_dir().ok_or_else(|| StoreError::Storage {
            detail: "logging not initialised".into(),
        })?;
        Ok(dir.display().to_string())
    }

    // ─── Sync conflicts ────────────────────────────────────────────────────────
    //
    // Field-level conflicts the sync applier recorded (a field edited differently
    // on two devices). Mirrors the desktop list/count/resolve commands; the
    // ConflictsRepo lives in host-core. No Tauri event bus on mobile → the UI
    // re-fetches after each resolve.

    /// Count of unresolved conflicts — the cheap badge query.
    pub fn sync_conflict_count(&self) -> Result<u32, StoreError> {
        let shared = self.db.shared();
        let repo = ConflictsRepo::new(&shared);
        // Self-heal: drop stored conflicts a newer build no longer considers
        // genuine (e.g. a recurrence serialization-drift conflict recorded before
        // the canonicalize fix) so the badge doesn't count a phantom.
        repo.prune_stale().map_err(map_conflicts_err)?;
        Ok(repo.unresolved_count().map_err(map_conflicts_err)? as u32)
    }

    /// Every unresolved conflict as a JSON `ConflictRecord[]` (the desktop
    /// `SyncConflict` wire shape).
    pub fn list_sync_conflicts_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = ConflictsRepo::new(&shared);
        // Self-heal before listing so a pre-fix spurious conflict (recurrence
        // serialization drift, a metadata field) never reaches the dialog.
        repo.prune_stale().map_err(map_conflicts_err)?;
        let records = repo.list_unresolved().map_err(map_conflicts_err)?;
        to_json(&records)
    }

    /// Apply the user's resolution for conflict `id`. `choice` is `"keep_local"`
    /// | `"take_remote"` | `"save_both"`. `keep_local` is pure bookkeeping (the
    /// merge already kept the local value); `take_remote` writes the remote value
    /// into the row + emits the `*Updated` event; `save_both` is not supported
    /// yet (matches desktop).
    pub fn resolve_sync_conflict(&self, id: i64, choice: String) -> Result<(), StoreError> {
        let parsed =
            ResolutionChoice::from_str(&choice).ok_or_else(|| StoreError::InvalidField {
                field: "choice".to_string(),
                detail: format!("unknown resolution choice '{choice}'"),
            })?;
        let shared = self.db.shared();
        let repo = ConflictsRepo::new(&shared);
        let record = repo.get(id).map_err(map_conflicts_err)?;
        match parsed {
            ResolutionChoice::KeepLocal => {
                repo.mark_resolved(id, parsed).map_err(map_conflicts_err)?;
            }
            ResolutionChoice::TakeRemote => {
                self.apply_take_remote(&record)?;
                repo.mark_resolved(id, parsed).map_err(map_conflicts_err)?;
            }
            ResolutionChoice::SaveBoth => {
                return Err(StoreError::Unsupported {
                    detail: "saving both versions is not supported yet".to_string(),
                });
            }
        }
        Ok(())
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
            let mut acc =
                host_core::reminders::enumerate_triggers(&shared, &self.registry, now, latest)
                    .await;
            // Synthetic birthday calendars aren't adapters, so the external
            // fan-out inside `enumerate_triggers` skips them. Fold their
            // configured default reminders in here, windowed to [now, latest]
            // the same way the external branch is.
            for t in host_core::reminders::enumerate_birthday_triggers(
                &self.adapter,
                &self.registry,
                &self.cache,
                &shared,
            )
            .await
            {
                if t.trigger_at >= now && t.trigger_at <= latest {
                    acc.push(t);
                }
            }
            acc
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
                    "container_id": t.container_id,
                    "title": t.title,
                    "body": t.body,
                    "trigger_at": t.trigger_at.to_rfc3339(),
                    // Occurrence start/end + all-day flag so the scheduler can build
                    // a localized all-day body ("Ganztägig · 24. Juni bis 26. Juni")
                    // instead of the meaningless "00:00" a midnight start formats to.
                    "start": t.start.to_rfc3339(),
                    "end": t.relevant_until.to_rfc3339(),
                    "all_day": t.all_day,
                    // The §14.4 effective sound (reminder→item→container→global),
                    // already resolved on the Trigger — the mobile scheduler maps
                    // it to the OS notification sound (System vs Silent).
                    "sound": t.sound,
                })
            })
            .collect();
        to_json(&dtos)
    }

    // ── Custom reminder sounds (§14.4 / §19.2.2) ──────────────────────────────
    //
    // The content-addressed audio store behind SoundSource::Custom. Files live
    // at `<data_dir>/assets/sounds/<sha256>.<ext>`; the sync round already
    // push/fetches them (DesktopSyncRoundHooks via build_orchestrator), so a
    // desktop-imported sound reaches this device and vice versa. These get a
    // user's file into the store + let the UI list / preview / delete it. Bytes
    // never cross UniFFI — the JS plays + builds the Android notification channel
    // from the on-disk PATH. (iOS can't use a runtime file as a notification
    // sound, so there it previews only.)

    /// Import an audio file into the custom-sound store (the JS picked it via
    /// expo-document-picker; `path` is its local path). Returns JSON
    /// `{sha256, ext, path}` (path = the stored asset's absolute path, for an
    /// immediate preview). Validates format + size via the shared importer.
    pub fn import_sound_json(&self, path: String) -> Result<String, StoreError> {
        let imported =
            host_core::sound_assets::import_sound(&self.sounds_dir, std::path::Path::new(&path))
                .map_err(map_import_err)?;
        let abs = self
            .sounds_dir
            .join(format!("{}.{}", imported.sha256, imported.ext));
        to_json(&serde_json::json!({
            "sha256": imported.sha256,
            "ext": imported.ext,
            "path": abs.to_string_lossy(),
        }))
    }

    /// Every custom sound in the store as JSON `[{sha256, ext, path}]`.
    pub fn list_custom_sounds_json(&self) -> Result<String, StoreError> {
        let list = host_core::sound_assets::list_local_sounds(&self.sounds_dir).map_err(|e| {
            StoreError::Storage {
                detail: e.to_string(),
            }
        })?;
        let dtos: Vec<serde_json::Value> = list
            .into_iter()
            .map(|(sha256, ext)| {
                let abs = self.sounds_dir.join(format!("{sha256}.{ext}"));
                serde_json::json!({
                    "sha256": sha256,
                    "ext": ext,
                    "path": abs.to_string_lossy(),
                })
            })
            .collect();
        to_json(&dtos)
    }

    /// The absolute on-disk path of a custom sound by hash, or `None` when it's
    /// not present locally (not yet synced / deleted) — the scheduler then falls
    /// back to the system sound. Drives preview + the Android channel sound.
    pub fn custom_sound_path(&self, sha256: String) -> Result<Option<String>, StoreError> {
        Ok(
            host_core::sound_assets::local_sound_path(&self.sounds_dir, &sha256)
                .map(|p| p.to_string_lossy().into_owned()),
        )
    }

    /// Delete a custom sound from the store by hash. Idempotent (a missing file
    /// is already-gone). Prefs still referencing it fall back to System at
    /// resolve time. Mirrors the desktop `delete_custom_sound`.
    pub fn delete_custom_sound(&self, sha256: String) -> Result<(), StoreError> {
        if let Some(path) = host_core::sound_assets::local_sound_path(&self.sounds_dir, &sha256) {
            std::fs::remove_file(&path).map_err(storage_err)?;
        }
        Ok(())
    }

    // ── Collaboration: RSVP (§7.3) + task-list members/sharing (§9.7) ─────────
    //
    // All routed to the owning EXTERNAL adapter — the local store has no user or
    // sharing concept. Read paths degrade to empty / null for local + unroutable
    // accounts (matching the desktop commands), so the UI simply hides the
    // affordance; write paths surface an error. These bypass the SWR cache (a
    // members/shares list is small + rarely changes; the desktop reads them live
    // too), except `respond_to_event`, which invalidates the event snapshot so
    // the new status shows on the next read.

    /// The connected account's email for `calendar_id`, used by the RSVP UI to
    /// tell an *attendee* from the *organizer*. `None` for local/iCal calendars
    /// and any provider that can't report an identity (which hides RSVP).
    /// Mirrors the desktop `calendar_current_user_email`.
    pub fn calendar_current_user_email(
        &self,
        calendar_id: String,
    ) -> Result<Option<String>, StoreError> {
        let Some(ext) = self.route(&calendar_id).ok().flatten() else {
            return Ok(None);
        };
        Ok(self
            .runtime
            .block_on(async { ext.current_user_email().await })
            .unwrap_or(None))
    }

    /// RSVP to an invitation on `calendar_id` / `event_id`: set the connected
    /// user's participation status. `status` is the wire `AttendeeStatus`
    /// (kebab-case). When `send_response` is true a scheduling-capable provider
    /// also emails the reply to the organizer. Invalidates the calendar's event
    /// cache so the next read reflects the new status. Local / unroutable →
    /// error (the UI only offers RSVP on scheduling-capable, non-organizer
    /// meetings). Mirrors the desktop `respond_to_event`.
    pub fn respond_to_event(
        &self,
        calendar_id: String,
        event_id: String,
        status: String,
        send_response: bool,
    ) -> Result<(), StoreError> {
        let status = parse_attendee_status(&status)?;
        let ext = self.route(&calendar_id)?.ok_or(StoreError::Unsupported {
            detail: "RSVP is only available on external calendar accounts".to_string(),
        })?;
        self.runtime
            .block_on(async { ext.respond_to_event(&event_id, status, send_response).await })
            .map_err(map_store_err)?;
        self.invalidate_events_cache(&calendar_id);
        Ok(())
    }

    /// Attendee free/busy over `[range_start, range_end]` for the account owning
    /// the request's `calendar_id`, as a JSON `FreeBusy[]` (`{email, slots:
    /// [{start, end}]}`). Best-effort + non-blocking on failure: a LOCAL
    /// calendar, an unroutable account, or a provider that can't answer (no
    /// scheduling / permission denied) returns `[]` — the UI reads that as
    /// "free/unknown" rather than an error. Mirrors the desktop `query_free_busy`.
    pub fn query_free_busy_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: FreeBusyRequest = from_json("request", &request_json)?;
        let Some(ext) = self.route(&req.calendar_id).ok().flatten() else {
            return to_json(&Vec::<cal_core::FreeBusy>::new());
        };
        let range = DateRange::new(req.range_start, req.range_end);
        let refs: Vec<&str> = req.emails.iter().map(String::as_str).collect();
        let fb = self
            .runtime
            .block_on(async { ext.get_free_busy(&refs, range).await })
            .unwrap_or_default();
        to_json(&fb)
    }

    /// Users assignable to a task in `list_id` — its collaborator pool (§9.7),
    /// as a JSON `TaskUser[]`. Empty for local lists / providers without
    /// sharing. Mirrors the desktop `task_list_members`.
    pub fn task_list_members_json(&self, list_id: String) -> Result<String, StoreError> {
        let members = match self.route_task_list(&list_id).ok().flatten() {
            None => Vec::new(),
            Some(ext) => self
                .runtime
                .block_on(async { ext.list_task_list_members(&list_id).await })
                .map_err(map_store_err)?,
        };
        to_json(&members)
    }

    /// The connected account's own identity ("me") for the account owning
    /// `list_id`, as a JSON `TaskUser` (or `null`). Lets the assignee picker mark
    /// "assigned to me". `null` for local lists. Mirrors `task_current_user`.
    pub fn task_current_user_json(&self, list_id: String) -> Result<String, StoreError> {
        let me = match self.route_task_list(&list_id).ok().flatten() {
            None => None,
            Some(ext) => self
                .runtime
                .block_on(async { ext.current_user().await })
                .map_err(map_store_err)?,
        };
        to_json(&me)
    }

    /// The editable membership/shares of `list_id` as a JSON `TaskListShare[]`
    /// (§9.7), driving the members manager. Empty for local / non-manageable
    /// backends. Mirrors the desktop `task_list_shares`.
    pub fn task_list_shares_json(&self, list_id: String) -> Result<String, StoreError> {
        let shares = match self.route_task_list(&list_id).ok().flatten() {
            None => Vec::new(),
            Some(ext) => self
                .runtime
                .block_on(async { ext.list_task_list_shares(&list_id).await })
                .map_err(map_store_err)?,
        };
        to_json(&shares)
    }

    /// Search the owning account's user directory for people to add to `list_id`
    /// (Vikunja) as a JSON `TaskUser[]`. Empty for local lists and backends
    /// without a directory (Todoist invites by raw email). Mirrors the desktop
    /// `task_search_users`.
    pub fn task_search_users_json(
        &self,
        list_id: String,
        query: String,
    ) -> Result<String, StoreError> {
        let users = match self.route_task_list(&list_id).ok().flatten() {
            None => Vec::new(),
            Some(ext) => self
                .runtime
                .block_on(async { ext.search_users(&query).await })
                .map_err(map_store_err)?,
        };
        to_json(&users)
    }

    /// Add/invite a member to `list_id`. `member_ref` is the provider's add key
    /// (Vikunja username, Todoist email). `right` ("read" / "write" / "admin")
    /// applies on backends with roles; `None` where the backend has none
    /// (Todoist). Mirrors the desktop `task_add_member`.
    pub fn task_add_member(
        &self,
        list_id: String,
        member_ref: String,
        right: Option<String>,
    ) -> Result<(), StoreError> {
        let right = right.map(|r| parse_member_right(&r)).transpose()?;
        let ext = self.route_task_list_external(&list_id)?;
        self.runtime
            .block_on(async { ext.add_task_list_member(&list_id, &member_ref, right).await })
            .map_err(map_store_err)
    }

    /// Remove a member from `list_id`. `member_ref` is the provider's remove key
    /// (Vikunja user id, Todoist email). Mirrors the desktop `task_remove_member`.
    pub fn task_remove_member(
        &self,
        list_id: String,
        member_ref: String,
    ) -> Result<(), StoreError> {
        let ext = self.route_task_list_external(&list_id)?;
        self.runtime
            .block_on(async { ext.remove_task_list_member(&list_id, &member_ref).await })
            .map_err(map_store_err)
    }

    /// Change an existing member's right on `list_id` (Vikunja). `right` is the
    /// wire `MemberRight` (snake_case). Mirrors the desktop `task_set_member_right`.
    pub fn task_set_member_right(
        &self,
        list_id: String,
        member_ref: String,
        right: String,
    ) -> Result<(), StoreError> {
        let right = parse_member_right(&right)?;
        let ext = self.route_task_list_external(&list_id)?;
        self.runtime
            .block_on(async {
                ext.set_task_list_member_right(&list_id, &member_ref, right)
                    .await
            })
            .map_err(map_store_err)
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

    // ── Day markers (what a day was like; local-only, always synced) ─────────
    //
    // Same shape as the colour labels below: no external provider models "how
    // was Tuesday", so every mutation is LOCAL and unconditionally appends a
    // sync event. Mirrors the desktop day_markers commands one for one.

    /// The marker vocabulary as a JSON `DayMarker[]`, in the user's order.
    pub fn list_day_markers_json(&self) -> Result<String, StoreError> {
        let markers = self.adapter.list_day_markers().map_err(map_store_err)?;
        to_json(&markers)
    }

    /// Add a marker; returns the created `DayMarker` as JSON.
    pub fn create_day_marker_json(
        &self,
        name: String,
        symbol: Option<String>,
        color_label: Option<String>,
    ) -> Result<String, StoreError> {
        let color = color_label.map(cal_core::ColorLabelId::new);
        let marker = self
            .adapter
            .create_day_marker(&name, symbol.as_deref(), color.as_ref())
            .map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&marker) {
            self.writer
                .append(SyncEvent::DayMarkerWritten(EventPayload {
                    id: marker.id.clone(),
                    fields,
                }));
        }
        to_json(&marker)
    }

    /// Write a marker back whole — rename, re-symbol, recolour, reorder are
    /// one call, so the frontend needs one code path for all four.
    pub fn update_day_marker_json(&self, marker_json: String) -> Result<String, StoreError> {
        let marker: cal_core::DayMarker = from_json("marker", &marker_json)?;
        let stamped = cal_core::DayMarker {
            updated_at: chrono::Utc::now(),
            ..marker
        };
        self.adapter
            .write_day_marker(&stamped)
            .map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&stamped) {
            self.writer
                .append(SyncEvent::DayMarkerWritten(EventPayload {
                    id: stamped.id.clone(),
                    fields,
                }));
        }
        to_json(&stamped)
    }

    pub fn delete_day_marker(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_day_marker(&id).map_err(map_store_err)?;
        self.writer
            .append(SyncEvent::DayMarkerDeleted(IdPayload { id }));
        Ok(())
    }

    /// One day's log as JSON. An untouched day comes back as an empty log,
    /// never null — the callers render the same thing either way.
    pub fn day_log_json(&self, day: String) -> Result<String, StoreError> {
        let parsed = parse_day_key(&day)?;
        let log = self.adapter.day_log(parsed).map_err(map_store_err)?;
        to_json(&log)
    }

    /// Every logged day in an inclusive range — what a week or month view asks
    /// for once, instead of one call per day.
    pub fn day_logs_in_range_json(&self, from: String, to: String) -> Result<String, StoreError> {
        let logs = self
            .adapter
            .day_logs_in_range(parse_day_key(&from)?, parse_day_key(&to)?)
            .map_err(map_store_err)?;
        to_json(&logs)
    }

    /// Set a day's log. Emitted even when it empties the day: the receiving
    /// side deletes the row for an empty log, which is how "I unticked the
    /// last one" reaches the other device.
    pub fn set_day_log_json(&self, log_json: String) -> Result<String, StoreError> {
        let log: cal_core::DayLog = from_json("log", &log_json)?;
        let stamped = cal_core::DayLog {
            updated_at: chrono::Utc::now(),
            ..log
        };
        self.adapter.set_day_log(&stamped).map_err(map_store_err)?;
        if let Ok(fields) = serde_json::to_value(&stamped) {
            self.writer.append(SyncEvent::DayLogSet(EventPayload {
                id: stamped.day.format("%Y-%m-%d").to_string(),
                fields,
            }));
        }
        to_json(&stamped)
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

    // ── Event groups (which events mean the same appointment) ────────────────
    //
    // A group is Aperio's statement ABOUT foreign data and reaches no provider,
    // so — like colour labels — every mutation is local and always emits. See
    // `DESIGN-event-groups.md` and the desktop `event_groups` commands.

    /// Declare that these events mean the same appointment.
    ///
    /// Takes a JSON array of `{calendar_id, event_id, title, starts_at}` and
    /// returns the resulting group as JSON. `title`/`starts_at` are the
    /// SIGNATURE: what the event looked like when it joined, kept so a member
    /// whose provider id changes can be found again.
    pub fn group_events_json(&self, members_json: String) -> Result<String, StoreError> {
        #[derive(serde::Deserialize)]
        struct Incoming {
            calendar_id: String,
            event_id: String,
            title: String,
            starts_at: String,
        }
        let incoming: Vec<Incoming> = from_json("event group members", &members_json)?;
        let members: Vec<NewMember> = incoming
            .into_iter()
            .map(|m| NewMember {
                calendar_id: m.calendar_id,
                event_id: m.event_id,
                title: m.title,
                starts_at: m.starts_at,
            })
            .collect();
        let shared = self.db.shared();
        let grouped = EventGroupsRepo::new(&shared)
            .group(&members)
            .map_err(map_group_err)?;
        self.emit_event_group(&grouped.group);
        // The refusals this took back have to travel too: a clearing that
        // stays on this device is a mark the others still hold — and they
        // would use it to break the very group just made.
        self.emit_declines(&grouped.cleared);
        to_json(&grouped.group)
    }

    /// Take one event out of its group. Returns the group as it stands
    /// afterwards, or `None` when the removal dissolved it (fewer than two
    /// members left) or the event was not grouped at all.
    ///
    /// `bookkeeping` says this is NOT the user taking something out — see
    /// `Removal`. The series carry sets it, because a copy it takes out is on
    /// its way straight back into the new group.
    pub fn ungroup_event_json(
        &self,
        calendar_id: String,
        event_id: String,
        bookkeeping: bool,
    ) -> Result<Option<String>, StoreError> {
        let shared = self.db.shared();
        let removal = if bookkeeping {
            Removal::Bookkeeping
        } else {
            Removal::ByUser
        };
        let outcome = EventGroupsRepo::new(&shared)
            .ungroup(&calendar_id, &event_id, removal)
            .map_err(map_group_err)?;
        match outcome {
            Some(Ungrouped::Remains { group, declines }) => {
                self.emit_event_group(&group);
                self.emit_declines(&declines);
                Ok(Some(to_json(&group)?))
            }
            Some(Ungrouped::Dissolved { group_id, declines }) => {
                self.writer
                    .append(SyncEvent::EventGroupDissolved(IdPayload { id: group_id }));
                self.emit_declines(&declines);
                Ok(None)
            }
            // Not grouped: nothing happened, so nothing is told.
            None => Ok(None),
        }
    }

    /// Dissolve a whole group. The events themselves are untouched.
    pub fn dissolve_event_group(&self, group_id: String) -> Result<(), StoreError> {
        let shared = self.db.shared();
        if let Some(declines) = EventGroupsRepo::new(&shared)
            .dissolve(&group_id)
            .map_err(map_group_err)?
        {
            self.writer
                .append(SyncEvent::EventGroupDissolved(IdPayload { id: group_id }));
            self.emit_declines(&declines);
        }
        Ok(())
    }

    /// Write down what a member's event looks like now, so it can still be
    /// found after the provider remints its id. Local and silent — see
    /// `EventGroupsRepo::refresh_signature`.
    pub fn refresh_event_group_signature(
        &self,
        calendar_id: String,
        event_id: String,
        title: String,
        starts_at: String,
    ) -> Result<(), StoreError> {
        let shared = self.db.shared();
        EventGroupsRepo::new(&shared)
            .refresh_signature(&calendar_id, &event_id, &title, &starts_at)
            .map_err(map_group_err)
    }

    /// Record that two events are NOT the same appointment, so Aperio stops
    /// offering to group them. Takes the two refs as JSON `{calendar_id,
    /// event_id}` objects.
    pub fn decline_group_suggestion_json(
        &self,
        first_json: String,
        second_json: String,
    ) -> Result<(), StoreError> {
        #[derive(serde::Deserialize)]
        struct Ref {
            calendar_id: String,
            event_id: String,
        }
        let first: Ref = from_json("event ref", &first_json)?;
        let second: Ref = from_json("event ref", &second_json)?;
        let shared = self.db.shared();
        let decline = EventGroupsRepo::new(&shared)
            .decline_suggestion(
                (&first.calendar_id, &first.event_id),
                (&second.calendar_id, &second.event_id),
            )
            .map_err(map_group_err)?;
        self.emit_declines(std::slice::from_ref(&decline));
        Ok(())
    }

    /// Every pair the user has said is not one appointment, as JSON.
    pub fn group_suggestion_declines_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let declines = EventGroupsRepo::new(&shared)
            .declined_suggestions()
            .map_err(map_group_err)?;
        to_json(&declines)
    }

    /// One member, found again under the id its event carries now.
    ///
    /// Silent on purpose: it repairs Aperio's own bookkeeping and changes
    /// nothing about which events mean the same appointment. See
    /// `EventGroupsRepo::heal_member`.
    pub fn heal_event_group_member(
        &self,
        group_id: String,
        calendar_id: String,
        old_event_id: String,
        new_event_id: String,
    ) -> Result<(), StoreError> {
        let shared = self.db.shared();
        // Local only: every device has the same evidence and repairs itself.
        // See `EventGroupsRepo::heal_member` for why broadcasting it was
        // harmful.
        EventGroupsRepo::new(&shared)
            .heal_member(&group_id, &calendar_id, &old_event_id, &new_event_id)
            .map_err(map_group_err)?;
        Ok(())
    }

    /// Every group any of these events belongs to, as a JSON `EventGroup[]`.
    ///
    /// Takes a JSON array of `{calendar_id, event_id}`. Groups come back WHOLE,
    /// including members outside the rendered range — a group only reads as a
    /// whole ("this and three others").
    pub fn event_groups_for_events_json(&self, events_json: String) -> Result<String, StoreError> {
        #[derive(serde::Deserialize)]
        struct Incoming {
            calendar_id: String,
            event_id: String,
        }
        let incoming: Vec<Incoming> = from_json("event refs", &events_json)?;
        let refs: Vec<(String, String)> = incoming
            .into_iter()
            .map(|e| (e.calendar_id, e.event_id))
            .collect();
        let shared = self.db.shared();
        let groups = EventGroupsRepo::new(&shared)
            .groups_for_events(&refs)
            .map_err(map_group_err)?;
        to_json(&groups)
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
    /// source name stays the single truth). A contact list renames its own row
    /// at the source instead — the local store or the provider (every adapter
    /// implements `rename_contact_list`); contacts aren't event-logged, so there
    /// is no sync event or override. `kind` is `"calendar"` | `"task_list"` |
    /// `"contact_list"`.
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
            // Address book (local or external): rename its own row at the source.
            // Routed by list id — the local store, else the provider adapter; both
            // implement `rename_contact_list`. Contacts aren't event-logged, so no
            // sync event / override (the source name is the single truth, seen on
            // the next read on every device).
            "contact_list" => {
                let route = self.route_contact_list(&container_id)?;
                let is_external = route.is_some();
                self.runtime
                    .block_on(async {
                        match route {
                            None => {
                                self.adapter
                                    .rename_contact_list(&container_id, trimmed)
                                    .await
                            }
                            Some(ext) => ext.rename_contact_list(&container_id, trimmed).await,
                        }
                    })
                    .map_err(map_store_err)?;
                // An EXTERNAL book's catalogue row now carries the new name at the
                // source, so the cached contact-list listing is stale — invalidate
                // it so the next `contact_lists_json` re-fetches the new name. (A
                // LOCAL book is read live, never cached, so nothing to invalidate.)
                if is_external {
                    if let Some(account) = self.registry.account_for_contact_list(&container_id) {
                        self.invalidate_contact_lists_listing(&account);
                    }
                }
                Ok(())
            }
            // External calendar / task list: push to the provider first, override
            // only on Unsupported.
            _ => {
                let ck = parse_container_kind(&kind)?;
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
                    // Source accepted it — drop any stale override (non-fatal) and
                    // invalidate the cached catalogue so the next listing re-fetches
                    // the new name (the provider's listing row changed, but the
                    // cached row still has the old name; the read-time override is
                    // cleared, so without this the stale cached name would win).
                    Ok(()) => {
                        let _ = repo.clear(&container_id, ck);
                        match ck {
                            ContainerKind::Calendar => self.invalidate_calendars_listing(&account),
                            ContainerKind::TaskList => self.invalidate_task_lists_listing(&account),
                            // Address books never reach this arm (the source-rename
                            // path above returns Unsupported for them).
                            ContainerKind::ContactList => {
                                self.invalidate_contact_lists_listing(&account)
                            }
                        }
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
        // The colour is a host-local override stamped at read-time by
        // `apply_color_to_sections`, so it surfaces on the next read regardless
        // of the section cache — but invalidate anyway to stay consistent with
        // the other section mutations (and any future provider-side colour).
        self.invalidate_sections_cache(&list_id);
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
            // External contact lists via SWR, per account — same shape as the
            // calendar listing (see list_calendars_json): a WARM account serves
            // its cached catalogue instantly (offline, no provider round-trip
            // gating the UI); a COLD account serves its RETAINED catalogue when one
            // exists, and only a never-listed account fetches live. Prime routes from whatever we serve, and
            // background-refresh (which also primes routes + caches) when cold or
            // stale. Per-adapter errors are swallowed so one dead account can't
            // blank the whole list.
            let mut external: Vec<ContactList> = Vec::new();
            for (account, adapter) in self.registry.snapshot_contact_adapters() {
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::ContactLists, "")
                    .ok()
                    .flatten();
                let warm = has_snapshot(&state);
                let stale = is_stale(&state, SWR_TTL_SECS);
                // Cache-only serve — a never-listed account contributes
                // NOTHING until the spawned refresh below lands. Mirrors
                // list_calendars_json.
                let lists = self.cache.read_contact_lists(&account).unwrap_or_default();
                for l in &lists {
                    self.registry.note_contact_list_route(&l.id, &account);
                }
                if !warm || stale {
                    let adapter_bg = Arc::clone(&adapter);
                    let reg = Arc::clone(&self.registry);
                    let acc = account.clone();
                    spawn_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::ContactLists,
                        account.clone(),
                        String::new(),
                        move || async move { adapter_bg.list_contact_lists().await },
                        move |c, list: &[ContactList]| {
                            for l in list {
                                reg.note_contact_list_route(&l.id, &acc);
                            }
                            c.replace_contact_lists(&acc, list)
                        },
                    );
                }
                external.extend(lists);
            }
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
        match self.route_contact_list(&list_id)? {
            // LOCAL: a direct read (the local store isn't cached).
            None => {
                let contacts = self
                    .runtime
                    .block_on(async { self.adapter.get_contacts(&list_id).await })
                    .map_err(map_store_err)?;
                to_json(&contacts)
            }
            // EXTERNAL: stale-while-revalidate — see get_events_json for the
            // full rationale. A WARM book (a snapshot exists) serves the
            // cached rows instantly + queues a background refresh when
            // stale; a COLD one serves whatever rows are retained (empty on
            // a never-warmed book) and lets the spawned refresh fill the
            // view via the cache-updated push. Mirrors the desktop contacts
            // read.
            Some(ext) => {
                let account = self
                    .registry
                    .account_for_contact_list(&list_id)
                    .unwrap_or_else(|| LOCAL_ID.to_string());
                let state = self
                    .cache
                    .get_sync_state(&account, SyncScope::Contacts, &list_id)
                    .ok()
                    .flatten();
                let warm = has_snapshot(&state);
                let stale = is_stale(&state, SWR_TTL_SECS);
                // Cache-only serve — a never-warmed book serves EMPTY and the
                // spawned refresh fills the view via the cache-updated push
                // (see get_events_json; the old blocking live read stalled
                // the serial native queue at first-paint time).
                let contacts = self
                    .cache
                    .read_contacts(&account, &list_id)
                    .unwrap_or_default();
                if !warm || stale {
                    let cache_bg = Arc::clone(&self.cache);
                    let ext_bg = Arc::clone(&ext);
                    let acc = account.clone();
                    let list = list_id.clone();
                    spawn_item_refresh(
                        self.runtime.handle(),
                        self.observer(),
                        Arc::clone(&self.cache),
                        Arc::clone(&self.coord),
                        SyncScope::Contacts,
                        account,
                        list_id.clone(),
                        move || async move {
                            refresh_contacts(&cache_bg, ext_bg.as_ref(), &acc, &list).await
                        },
                    );
                }
                to_json(&contacts)
            }
        }
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
        self.invalidate_contacts_cache(&contact.list_id);
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
        // The update API carries no previous list (a contact can't move between
        // books in one call here, unlike a task/event move), so only the
        // contact's own list needs invalidating.
        self.invalidate_contacts_cache(&updated.list_id);
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
            .map_err(map_store_err)?;
        if let Some(lid) = list_id.as_deref() {
            self.invalidate_contacts_cache(lid);
        }
        Ok(())
    }

    /// A contact's avatar as JSON `Option<ContactPhoto>` — `{content_type,
    /// data:<base64>}` or `null` when the contact has no photo — routed by the
    /// optional `list_id` (omit → local). The listing's `has_photo` flag drives
    /// whether the UI calls this. Mirrors the desktop `get_contact_photo`.
    pub fn get_contact_photo_json(
        &self,
        id: String,
        list_id: Option<String>,
    ) -> Result<String, StoreError> {
        let route = match list_id.as_deref() {
            Some(lid) => self.route_contact_list(lid)?,
            None => None,
        };
        let photo = self
            .runtime
            .block_on(async {
                match route {
                    None => self.adapter.get_contact_photo(&id).await,
                    Some(ext) => ext.get_contact_photo(&id).await,
                }
            })
            .map_err(map_store_err)?;
        to_json(&photo)
    }

    /// Set (or replace) a contact's avatar from a JSON `ContactPhoto`
    /// (`{content_type, data:<base64>}`), routed by the optional `list_id`. An
    /// adapter that doesn't model photos surfaces `Unsupported`. Mirrors the
    /// desktop `set_contact_photo`.
    pub fn set_contact_photo_json(
        &self,
        id: String,
        list_id: Option<String>,
        photo_json: String,
    ) -> Result<(), StoreError> {
        let photo: cal_core::ContactPhoto = from_json("photo", &photo_json)?;
        let route = match list_id.as_deref() {
            Some(lid) => self.route_contact_list(lid)?,
            None => None,
        };
        self.runtime
            .block_on(async {
                match route {
                    None => self.adapter.set_contact_photo(&id, photo).await,
                    Some(ext) => ext.set_contact_photo(&id, photo).await,
                }
            })
            .map_err(map_store_err)?;
        // A photo change alters the cached contact — invalidate so the next read
        // re-fetches it.
        if let Some(lid) = list_id.as_deref() {
            self.invalidate_contacts_cache(lid);
        }
        Ok(())
    }

    /// Remove a contact's avatar (other fields untouched), routed by the
    /// optional `list_id`. Mirrors the desktop `delete_contact_photo`.
    pub fn delete_contact_photo(
        &self,
        id: String,
        list_id: Option<String>,
    ) -> Result<(), StoreError> {
        let route = match list_id.as_deref() {
            Some(lid) => self.route_contact_list(lid)?,
            None => None,
        };
        self.runtime
            .block_on(async {
                match route {
                    None => self.adapter.delete_contact_photo(&id).await,
                    Some(ext) => ext.delete_contact_photo(&id).await,
                }
            })
            .map_err(map_store_err)?;
        // A photo change alters the cached contact — invalidate so the next read
        // re-fetches it.
        if let Some(lid) = list_id.as_deref() {
            self.invalidate_contacts_cache(lid);
        }
        Ok(())
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

    // ── Schema-driven accounts ───────────────────────────────────────────────
    //
    // The generic path: an adapter publishes an account schema in its
    // `plugin.json`, and the host executes it. Nothing below names a provider,
    // and adding an adapter adds no code here. The desktop twin lives in
    // `src-tauri/src/commands/accounts.rs`; both call the same
    // `host_core::account_setup`, so a posture decided on one platform reads
    // back identically on the other.

    /// The connect form an adapter declares, as JSON, or `null` when it
    /// declares none.
    ///
    /// The `builtin` flag inside the OAuth block is resolved HERE rather than
    /// in the UI: it is a question about what this build carries, which the
    /// frontend cannot see and should never be handed.
    pub fn account_form_spec_json(
        &self,
        adapter_kind: String,
        lang: Option<String>,
    ) -> Result<String, StoreError> {
        // No "unknown kind" branch: which kinds exist is a fact about which
        // plugins are loaded, so an unrecognised one is simply a plugin that
        // declares no form — the same answer as an adapter still on the older
        // per-kind path.
        let Some(plugin) = self.plugin_manager.plugin_for_adapter_kind(&adapter_kind) else {
            return Ok("null".to_string());
        };
        let plugin_id = plugin.manifest.id.clone();
        let Some(schema) = plugin.manifest.account.clone() else {
            return Ok("null".to_string());
        };
        // Labels resolved HERE, in the language the caller named, against the
        // plugin's own catalogue. The frontend renders what it is given and
        // never looks a plugin's key up in the app's translations — the app
        // carries no word about somebody else's provider.
        let lang = lang.as_deref().unwrap_or(plugin_core::FALLBACK_LANG);
        let strings = plugin_core::manager::PluginManager::strings_for(&plugin, lang);
        let fields: Vec<serde_json::Value> = schema
            .fields
            .iter()
            .map(|f| {
                let label = plugin_core::resolve_label(
                    Some(&strings),
                    f.label_key.as_deref(),
                    &f.label,
                    lang,
                );
                let hint = f
                    .hint
                    .as_deref()
                    .or(f.hint_key.as_deref().map(|_| ""))
                    .map(|verbatim| {
                        plugin_core::resolve_label(
                            Some(&strings),
                            f.hint_key.as_deref(),
                            verbatim,
                            lang,
                        )
                    })
                    .filter(|hint| !hint.is_empty());
                serde_json::json!({
                    "key": f.key,
                    "kind": f.kind,
                    "label": label,
                    "hint": hint,
                    "required": f.required,
                    "secret_slot": f.secret_slot,
                    "default": f.default,
                })
            })
            .collect();
        let resolve = |key: Option<&str>, verbatim: &str| {
            plugin_core::resolve_label(Some(&strings), key, verbatim, lang).to_string()
        };
        let optional = |value: Option<&String>, key: Option<&str>| {
            value
                .map(String::as_str)
                .or(key.map(|_| ""))
                .map(|verbatim| resolve(key, verbatim))
                .filter(|s| !s.is_empty())
        };
        let actions: Vec<serde_json::Value> = schema
            .actions
            .iter()
            .map(|a| {
                serde_json::json!({
                    "key": a.key,
                    "label": resolve(a.label_key.as_deref(), &a.label),
                    "busy_label": optional(a.busy_label.as_ref(), a.busy_label_key.as_deref()),
                    "success": optional(a.success.as_ref(), a.success_key.as_deref()),
                    "hint": optional(a.hint.as_ref(), a.hint_key.as_deref()),
                    "requires": a.requires.iter().map(|r| serde_json::json!({
                        "field": r.field,
                        "message": resolve(r.message_key.as_deref(), &r.message),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let spec = serde_json::json!({
            "plugin_id": plugin_id,
            "fields": fields,
            "actions": actions,
            "oauth": schema.oauth.as_ref().map(|o| serde_json::json!({
                "builtin": host_core::account_setup::has_builtin_client(o),
                "client_id_field": o.client_id_field,
                "client_secret_field": o.client_secret_field,
                "app_redirect_uri": o.app_redirect_uri,
            })),
            // Derived from the plugin's declared TYPE, so a frontend can skip
            // the catalog refresh for an adapter that owns no containers
            // without keeping its own list of which adapters those are.
            "owns_containers":
                plugin.manifest.has_data_family(),
            // Whether "test connection" can mean anything before the account
            // exists. Answered HERE rather than re-derived in each frontend, so
            // the button and the probe cannot disagree about it.
            "supports_credential_test":
                host_core::account_setup::supports_credential_test(&schema),
        });
        Ok(spec.to_string())
    }

    // ── Meetings ─────────────────────────────────────────────────────────────
    //
    // The mobile twins of the desktop `attach_meeting` / `detach_meeting` /
    // `event_meeting`. Same three steps in the same order and for the same
    // reasons: a meeting created without its link reaching the event is
    // invisible, and a link written without the binding recorded is a meeting
    // nobody can delete.

    /// Create a meeting for an event, write its link into the event, and record
    /// which meeting that was. Returns `{event, meeting}` as JSON.
    pub fn attach_meeting_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: AttachMeetingRequest = from_json("attach meeting", &request_json)?;
        let vc =
            self.registry
                .vc_adapter(&req.account_id)
                .ok_or_else(|| StoreError::InvalidField {
                    field: "account_id".to_string(),
                    detail: format!("no videoconference adapter for account {}", req.account_id),
                })?;

        let event = self
            .read_event_for_meeting(&req.event_id, &req.calendar_id)?
            .ok_or(StoreError::NotFound)?;

        let shared = self.db.shared();
        if MeetingsRepo::new(&shared)
            .get(&req.event_id)
            .map_err(meetings_err)?
            .is_some()
        {
            return Err(StoreError::InvalidField {
                field: "event_id".to_string(),
                detail: "this event already has a meeting — remove it first".to_string(),
            });
        }

        // Who is coming, and whether the provider is the one who has to tell
        // them. Addresses, not the display strings the event carries: a
        // provider validates this field as an email and refuses the meeting
        // otherwise. Mirrors the desktop `attach_meeting`.
        let can_invite = self.calendar_can_invite(&req.calendar_id);
        let guests = attendee_addresses(&event.attendees);
        let notify = should_provider_notify(&guests, can_invite);
        let meeting = self
            .runtime
            .block_on(vc.create_meeting(NewMeeting {
                title: event.title.clone(),
                start_time: Some(event.start),
                end_time: Some(event.end),
                description: event.description.clone(),
                use_personal_room: req.use_personal_room,
                attendees: guests.clone(),
                notify_attendees: notify,
            }))
            .map_err(vc_err)?;

        // The adapter named each line and supplied the values; its own
        // catalogue supplies the words, in the language the request asked for.
        // Mirrors the desktop `attach_meeting`.
        let mut updated = event.clone();
        let lang = req
            .invitation_lang
            .as_deref()
            .unwrap_or(plugin_core::FALLBACK_LANG);
        let catalogue = self.adapter_catalogue(&req.account_id);
        let block = cal_core::conferencing::meeting_block(&host_core::meetings::block_lines(
            &meeting.join_details,
            &meeting.join_url,
            catalogue.as_ref(),
            lang,
        ));
        updated.description = Some(match updated.description.as_deref().map(str::trim) {
            Some(existing) if !existing.is_empty() => format!("{existing}\n\n{block}"),
            _ => block,
        });
        if updated
            .location
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            updated.location = Some(meeting.join_url.clone());
        }
        // The complement of the provider decision, and the half that made the
        // rule work at all: exactly ONE channel announces the meeting. When the
        // calendar is the one that can, this write has to actually carry the
        // link to the attendees — the default is `false`, which on Exchange
        // means `SendToNone`, and then the provider stayed quiet AND the
        // calendar said nothing, so the join link reached nobody.
        updated.send_invitations = can_invite && !guests.is_empty();

        let saved = match self.save_event_for_meeting(updated) {
            Ok(saved) => saved,
            Err(err) => {
                // Nowhere for the meeting to live — take it back down rather
                // than leaving one behind that nothing on this device knows of.
                // The cancellation follows whatever the invitation did: if the
                // provider was asked to mail everyone a moment ago, those mails
                // are already out, and staying quiet now leaves people holding
                // an invitation to a meeting that no longer exists.
                if let Err(cleanup) = self.runtime.block_on(
                    vc.delete_meeting(vc_core::MeetingRemoval::new(meeting.id.clone(), notify)),
                ) {
                    tracing::warn!(
                        meeting_id = %meeting.id,
                        ?cleanup,
                        "could not roll back a meeting after the event failed to save"
                    );
                }
                return Err(err);
            }
        };

        MeetingsRepo::new(&shared)
            .bind(
                &req.event_id,
                &req.account_id,
                &meeting.id,
                &meeting.join_url,
            )
            .map_err(meetings_err)?;

        to_json(&serde_json::json!({
            "event": saved,
            "meeting": meeting,
        }))
    }

    /// Delete the meeting Aperio created for an event and take its link back
    /// out. Returns the saved event as JSON, or `null` when there was no
    /// meeting of ours — someone else's link is not ours to delete.
    pub fn detach_meeting_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: AttachMeetingRequest = from_json("detach meeting", &request_json)?;
        let shared = self.db.shared();
        let Some(binding) = MeetingsRepo::new(&shared)
            .get(&req.event_id)
            .map_err(meetings_err)?
        else {
            return Ok("null".to_string());
        };

        let vc = self
            .registry
            .vc_adapter(&binding.account_id)
            .ok_or_else(|| StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: format!(
                    "no videoconference adapter for account {}",
                    binding.account_id
                ),
            })?;
        // Whether the provider announces the cancellation. Deliberately not
        // asked of the event's attendee list: an adopted meeting has invitees
        // the event never knew, and by now the event may be gone or unreadable
        // — where "nobody" and "could not read" look identical. The provider
        // mails only the invitees it holds, so asking costs nothing when it
        // holds none. Mirrors the desktop `detach_meeting`.
        let can_invite = self.calendar_can_invite(&req.calendar_id);
        let notify = should_provider_announce_removal(can_invite);

        // Provider first: forgetting the id before the delete succeeds would
        // strand the meeting for good.
        // A permanent room cannot be deleted — the adapter answers `Unsupported`
        // — and unlinking is then the whole of what "remove" can mean. Any
        // other failure still aborts, since forgetting the id of a meeting that
        // does exist would strand it.
        match self
            .runtime
            .block_on(vc.delete_meeting(vc_core::MeetingRemoval::new(
                binding.meeting_id.clone(),
                notify,
            ))) {
            Ok(()) => {}
            Err(vc_core::VcError::Unsupported(reason)) => {
                tracing::info!(%reason, "unlinking a meeting the provider will not delete");
            }
            Err(err) => return Err(vc_err(err)),
        }
        MeetingsRepo::new(&shared)
            .unbind(&req.event_id)
            .map_err(meetings_err)?;

        let Some(event) = self.read_event_for_meeting(&req.event_id, &req.calendar_id)? else {
            return Ok("null".to_string());
        };
        let mut updated = event.clone();
        updated.description = updated
            .description
            .as_deref()
            .map(|text| cal_core::conferencing::without_meeting_block(text, &binding.join_url))
            .filter(|text| !text.is_empty());
        if updated.location.as_deref().map(str::trim) == Some(binding.join_url.as_str()) {
            updated.location = None;
        }
        // The complement again: when the calendar is the channel that
        // announces, it has to announce that the link is gone too. Otherwise
        // the attendees keep a join URL that now leads nowhere.
        updated.send_invitations = can_invite && !attendee_addresses(&updated.attendees).is_empty();
        let saved = self.save_event_for_meeting(updated)?;
        to_json(&saved)
    }

    /// Everything known about the meeting on an event: whether it is ours, what
    /// the provider says about it, and who it says is invited. As JSON.
    ///
    /// The lookup goes through the JOIN LINK, the only identifier that reaches
    /// a calendar event, so this answers for meetings Aperio did not create.
    /// The desktop twin is `inspect_event_meeting`.
    pub fn inspect_event_meeting_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: AttachMeetingRequest = from_json("inspect meeting", &request_json)?;
        let shared = self.db.shared();
        // Any copy's meeting is this appointment's meeting: a group says these
        // events are one thing, so which copy the link was made from stops
        // being the coincidence that decides whether Join appears.
        let binding = MeetingsRepo::new(&shared)
            .get_including_copies(&req.calendar_id, &req.event_id)
            .map_err(meetings_err)?;

        if let Some(binding) = &binding {
            if let Some(vc) = self.registry.vc_adapter(&binding.account_id) {
                let meeting = self
                    .runtime
                    .block_on(vc.get_meeting(&binding.meeting_id))
                    .unwrap_or(None);
                return to_json(&serde_json::json!({
                    "binding": binding,
                    "meeting": meeting,
                    "account_id": binding.account_id,
                }));
            }
        }

        let none = serde_json::json!({
            "binding": binding,
            "meeting": serde_json::Value::Null,
            "account_id": serde_json::Value::Null,
        });
        let Some(event) = self.read_event_for_meeting(&req.event_id, &req.calendar_id)? else {
            return to_json(&none);
        };
        let Some(conference) =
            cal_core::conferencing::detect_conference(&cal_core::conferencing::ConferenceSources {
                location: event.location.as_deref(),
                description: event.description.as_deref(),
                ..Default::default()
            })
        else {
            return to_json(&none);
        };
        for (account_id, vc) in self.registry.snapshot_vc_adapters() {
            match self
                .runtime
                .block_on(vc.resolve_meeting(&conference.join_url))
            {
                Ok(Some(meeting)) => {
                    return to_json(&serde_json::json!({
                        "binding": binding,
                        "meeting": meeting,
                        "account_id": account_id,
                    }))
                }
                // Not this account's, or this provider has no lookup by link.
                Ok(None) | Err(_) => continue,
            }
        }
        to_json(&none)
    }

    /// Take responsibility for a meeting Aperio did not create, so it can also
    /// be removed. Writes nothing to the event — the link is already there.
    pub fn adopt_meeting_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: AdoptMeetingRequest = from_json("adopt meeting", &request_json)?;
        let shared = self.db.shared();
        let bound = MeetingsRepo::new(&shared)
            .bind(
                &req.event_id,
                &req.account_id,
                &req.meeting_id,
                &req.join_url,
            )
            .map_err(meetings_err)?;
        to_json(&bound)
    }

    /// The meeting Aperio created for this event, if any, as JSON.
    pub fn event_meeting_json(
        &self,
        event_id: String,
        calendar_id: Option<String>,
    ) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = MeetingsRepo::new(&shared);
        // With the calendar known, a meeting attached to ANY copy answers.
        // Without it the plain binding is all there is.
        let found = match calendar_id {
            Some(cid) => repo.get_including_copies(&cid, &event_id),
            None => repo.get(&event_id),
        }
        .map_err(meetings_err)?;
        to_json(&found)
    }

    /// Every adapter this build knows, as JSON.
    ///
    /// Assembled from the loaded manifests rather than from a list in the UI:
    /// which adapters exist is decided by which plugins are embedded, and the
    /// connect picker has no business knowing that in advance.
    ///
    /// The built-in store rides along from its own manifest — see
    /// [`host_core::builtin_adapters`]. The device calendar does not: it exists
    /// only where the native bridge does and is added by granting a permission,
    /// so the accounts screen offers it on its own terms.
    pub fn list_adapter_kinds_json(&self) -> Result<String, StoreError> {
        to_json(&host_core::builtin_adapters::all_adapter_kinds(
            &self.plugin_manager,
        ))
    }

    /// Begin a schema-driven OAuth sign-in: build the consent URL for the
    /// adapter's own flow.
    ///
    /// `values_json` is the form as filled so far, keyed by the schema's field
    /// keys — the host reads the credential pair out of it and decides the
    /// posture. Returns the plugin's `{authorize_url, pkce_verifier, state}`
    /// for a native auth session to open.
    ///
    /// The posture is NOT remembered between this call and the completion: it
    /// is re-derived from the same values, which is deterministic for a given
    /// build, and means the host holds no cross-call credential state.
    pub fn begin_account_oauth_json(
        &self,
        adapter_kind: String,
        values_json: String,
    ) -> Result<String, StoreError> {
        let (plugin_id, schema) = self.schema_for(&adapter_kind)?;
        let oauth = schema
            .oauth
            .as_ref()
            .ok_or_else(|| StoreError::InvalidField {
                field: "adapter_kind".to_string(),
                detail: "this adapter does not sign in via OAuth".to_string(),
            })?;
        let values: serde_json::Map<String, serde_json::Value> = from_json("values", &values_json)?;
        let client = self.oauth_client_for(oauth, &values)?;
        let args = Self::auth_args(oauth, &values, &client, "authorize", &[]);
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

    /// Start a re-sign-in for an EXISTING account: the authorize URL, PKCE
    /// verifier and state, exactly as [`Self::begin_account_oauth_json`] but
    /// with the values taken from the account rather than a form.
    ///
    /// Works for any adapter that declares an `oauth` block. The pair it
    /// replaces took a plugin id and provider-shaped arguments and refused
    /// anything that was not Google or Microsoft Graph — so a Webex account
    /// whose grant expired could be created but never repaired.
    pub fn begin_account_reconnect_json(&self, account_id: String) -> Result<String, StoreError> {
        let (account, plugin_id, schema) = self.oauth_account(&account_id)?;
        let oauth = schema.oauth.as_ref().expect("checked in oauth_account");
        let values = self.reconnect_values(&account, oauth);
        // The AUTHORIZE step never carries the client secret — only the token
        // exchange does. On a device that does not hold the secret (a second
        // device, before a sync round delivers it), refusing here would block
        // the consent screen the user can perfectly well complete; the
        // missing secret is felt at the exchange, and
        // `complete_account_reconnect_json` can be handed it there.
        let client = match self.oauth_client_for(oauth, &values) {
            Ok(client) => client,
            Err(_) => match supplied_value(&values, &oauth.client_id_field) {
                Some(id) if !id.trim().is_empty() => host_core::account_setup::OauthClient {
                    id: id.trim().to_string(),
                    secret: None,
                },
                _ => {
                    return Err(StoreError::Sync {
                        code: "client_secret_required".into(),
                        detail: "this device holds neither the account's OAuth client secret nor its client id".into(),
                    })
                }
            },
        };
        let args = Self::auth_args(oauth, &values, &client, "authorize", &[]);
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
            detail: format!("authorize blob: {e}"),
        })
    }

    /// Finish a re-sign-in: exchange the code and write the fresh tokens under
    /// the EXISTING account id, so its calendars, colours and overrides survive.
    ///
    /// The row itself is untouched. Only the keychain moves, which is what makes
    /// this safe to retry: a failed exchange leaves the old (expired) tokens
    /// exactly where they were.
    pub fn complete_account_reconnect_json(
        &self,
        account_id: String,
        request_json: String,
    ) -> Result<String, StoreError> {
        let req: AccountReconnectRequest = from_json("account reconnect", &request_json)?;
        let (account, plugin_id, schema) = self.oauth_account(&account_id)?;
        let oauth = schema.oauth.as_ref().expect("checked in oauth_account");
        let mut values = self.reconnect_values(&account, oauth);
        let supplied_secret = req
            .client_secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let (Some(field), Some(secret)) =
            (oauth.client_secret_field.as_deref(), &supplied_secret)
        {
            values.insert(field.to_string(), serde_json::Value::String(secret.clone()));
        }
        // The whole choice, not just the client: `config` records WHICH
        // registration this account is now linked to, and a reconnect that
        // drops it cannot clear a stale one. See the write below.
        let choice = host_core::account_setup::choose_oauth_client(
            oauth,
            supplied_value(&values, &oauth.client_id_field).as_deref(),
            oauth
                .client_secret_field
                .as_deref()
                .and_then(|k| supplied_value(&values, k))
                .as_deref(),
        )
        .map_err(|err| {
            // The one refusal the caller can answer: type the client secret and
            // sign in again. A CODE, not prose — the JS side matches `error.code`,
            // and prose breaks the day it is reworded.
            if matches!(
                &err,
                host_core::account_setup::AccountSetupError::InvalidInput(m)
                    if m.contains("required alongside the client ID")
            ) {
                StoreError::Sync {
                    code: "client_secret_required".into(),
                    detail: err.to_string(),
                }
            } else {
                setup_err(err)
            }
        })?;
        let client = &choice.client;

        let exchange = Self::auth_args(
            oauth,
            &values,
            client,
            "exchange",
            &[
                ("code", serde_json::Value::String(req.code.clone())),
                (
                    "pkce_verifier",
                    serde_json::Value::String(req.pkce_verifier.clone()),
                ),
                ("state", serde_json::Value::String(req.state.clone())),
                (
                    "returned_state",
                    serde_json::Value::String(req.returned_state.clone()),
                ),
            ],
        );
        let bytes = self
            .runtime
            .block_on(async {
                self.plugin_manager
                    .interactive_auth(&plugin_id, &exchange.to_string())
                    .await
            })
            .map_err(|e| StoreError::Auth {
                detail: e.to_string(),
            })?;
        let tokens: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Protocol {
                detail: format!("token blob: {e}"),
            })?;

        // Only the tokens the plugin asked to be handed back. A provider that
        // returns no refresh token on a re-consent has left the account no
        // better off, so that is an error rather than a silent half-repair.
        let mut writes: Vec<(SecretSlot, String)> = Vec::new();
        // A typed client secret that just carried a successful exchange is
        // proven right — keep it, and offer it to credential sync so the next
        // device never has to ask.
        if let Some(secret) = &supplied_secret {
            writes.push((SecretSlot::OauthClientSecret, secret.clone()));
        }
        if oauth.refresh_token_field.is_some() {
            let refresh = tokens
                .get("refresh_token")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| StoreError::Protocol {
                    detail: "the provider returned no refresh token — the account could not be \
                             kept signed in"
                        .to_string(),
                })?
                .to_string();
            writes.push((SecretSlot::RefreshToken, refresh));
        }
        if oauth.access_token_field.is_some() {
            if let Some(access) = tokens
                .get("access_token")
                .and_then(serde_json::Value::as_str)
            {
                writes.push((SecretSlot::AccessToken, access.to_string()));
            }
        }

        let shared = self.db.shared();
        for (slot, value) in &writes {
            self.secret_store
                .store(&account.id, *slot, value)
                .map_err(|err| StoreError::Storage {
                    detail: format!("store {}: {err}", slot.wire_name()),
                })?;
            // E2E only: the user's other devices get the refreshed grant too.
            host_core::credential_sync::emit_credential_set(
                &self.writer,
                &shared,
                &self.plugin_manager,
                &account.id,
                *slot,
                value,
            );
        }
        // Record which registration the account is linked to NOW. Without this
        // a built-in-posture account survives a build whose OAuth client was
        // rotated only until the next open: the stored fingerprint no longer
        // matches, `resolve_oauth_client` refuses with "sign in again", and
        // signing in again wrote tokens while leaving the fingerprint stale —
        // an instruction that could never be carried out.
        let mut config: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&account.config_json).unwrap_or_default();
        for (key, value) in &choice.config {
            config.insert(key.clone(), value.clone());
        }
        // A bring-your-own account keeps its own id in the row; a built-in one
        // must NOT, or it would be pinned to whatever the build carried today.
        if choice.config.iter().any(|(k, _)| k == "client_source") {
            config.remove(&oauth.client_id_field);
        }
        let account = AccountsRepo::new(&shared)
            .set_config(&account.id, &serde_json::Value::Object(config).to_string())
            .map_err(acc_err)?;

        // The re-sign-in changed the stored config (the recorded OAuth client
        // posture) — say so on the wire like every other config write, or the
        // other devices keep a row that contradicts this one. BEFORE the
        // registration attempt: the row is committed either way, and a
        // registration hiccup must not leave the devices silently divergent.
        append_account_event(
            &self.writer,
            &self.plugin_manager,
            account.adapter_kind.as_str(),
            SyncEvent::AccountUpdated(account_payload(&account)),
        );
        // Live for the rest of the session without a restart. A failure leaves
        // the fresh tokens in place — retrying costs no second sign-in.
        self.registry
            .register(&account)
            .map_err(|err| StoreError::Storage {
                detail: format!("adapter registration failed: {err}"),
            })?;
        to_json(&account)
    }

    /// Finish a schema-driven connect: exchange the code if there is an OAuth
    /// block, then create the account.
    ///
    /// Works for BOTH shapes. An adapter with no OAuth block skips straight to
    /// the account creation, so the mobile UI has one call to make either way.
    /// Returns the created account as JSON.
    pub fn connect_account_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: ConnectAccountRequest = from_json("connect account", &request_json)?;
        let name = req.display_name.trim();
        if name.is_empty() {
            return Err(StoreError::InvalidField {
                field: "display_name".to_string(),
                detail: "display name must not be empty".to_string(),
            });
        }
        // EDIT of an existing account: the shared host-core implementation
        // (blank secret fields inherit the stored credential, non-form config
        // keys carry over, the live adapter re-registers, account.updated +
        // credential.set go on the wire). No OAuth exchange here — the client
        // pair stays the reconnect flow's job.
        if let Some(account_id) = req.account_id.as_deref() {
            let shared = self.db.shared();
            let updated = host_core::account_update::update_account_values(
                &shared,
                &self.registry,
                &self.plugin_manager,
                self.secret_store.as_ref(),
                &self.writer,
                account_id,
                Some(name),
                &req.values,
            )
            .map_err(|e| StoreError::InvalidField {
                field: "account".to_string(),
                detail: e.to_string(),
            })?;
            // No warm kick here: like the create path, the JS layer follows a
            // successful connect/edit with its own refreshExternalCache() —
            // the one owner of that trigger on mobile.
            return to_json(&updated);
        }

        let (plugin_id, schema) = self.schema_for(&req.adapter_kind)?;
        let kind = AdapterKind::new(req.adapter_kind.clone());

        // 1. Exchange FIRST, so a failed sign-in never leaves an orphaned row.
        let mut choice = None;
        let mut tokens = None;
        if let Some(oauth) = &schema.oauth {
            let client = self.oauth_client_for(oauth, &req.values)?;
            let exchange = Self::auth_args(
                oauth,
                &req.values,
                &client,
                "exchange",
                &[
                    ("code", serde_json::Value::String(req.code.clone())),
                    (
                        "pkce_verifier",
                        serde_json::Value::String(req.pkce_verifier.clone()),
                    ),
                    ("state", serde_json::Value::String(req.state.clone())),
                    (
                        "returned_state",
                        serde_json::Value::String(req.returned_state.clone()),
                    ),
                ],
            );
            let bytes = self
                .runtime
                .block_on(async {
                    self.plugin_manager
                        .interactive_auth(&plugin_id, &exchange.to_string())
                        .await
                })
                .map_err(|e| StoreError::Auth {
                    detail: e.to_string(),
                })?;
            tokens = Some(
                serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| {
                    StoreError::Protocol {
                        detail: format!("token blob: {e}"),
                    }
                })?,
            );
            choice = Some(
                host_core::account_setup::choose_oauth_client(
                    oauth,
                    supplied_value(&req.values, &oauth.client_id_field).as_deref(),
                    oauth
                        .client_secret_field
                        .as_deref()
                        .and_then(|k| supplied_value(&req.values, k))
                        .as_deref(),
                )
                .map_err(setup_err)?,
            );
        }

        // 2. Split the form into the row and the keychain writes.
        let mut plan =
            host_core::account_setup::plan_new_account(&schema, &req.values, choice.as_ref())
                .map_err(setup_err)?;

        // 3. The sign-in's tokens join the keychain writes, but only the ones
        //    the plugin asked to be handed back.
        let mut refresh_for_sync = None;
        if let (Some(oauth), Some(tokens)) = (&schema.oauth, &tokens) {
            if oauth.refresh_token_field.is_some() {
                let refresh = tokens
                    .get("refresh_token")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| StoreError::Protocol {
                        detail: "the provider returned no refresh token — the account could not \
                                 be kept signed in"
                            .to_string(),
                    })?
                    .to_string();
                plan.secrets
                    .push((SecretSlot::RefreshToken, refresh.clone()));
                refresh_for_sync = Some(refresh);
            }
            if oauth.access_token_field.is_some() {
                if let Some(access) = tokens
                    .get("access_token")
                    .and_then(serde_json::Value::as_str)
                {
                    plan.secrets
                        .push((SecretSlot::AccessToken, access.to_string()));
                }
            }
        }

        // 4. Persist: row, secrets, registration — unwinding all of it on any
        //    failure so a retry starts clean.
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let created = repo
            .create(kind, name, &plan.config_json)
            .map_err(acc_err)?;
        // This device's half, keyed by the id the row just got. A failure here
        // unwinds the row, the same way a failed secret write does below.
        if !plan.device_local.is_empty() {
            let prefs = UserPrefsRepo::new(&shared);
            let fields: Vec<String> = plan.device_local.keys().cloned().collect();
            if let Err(err) =
                host_core::account_local::store(&prefs, &created.id, &fields, &plan.device_local)
            {
                let _ = repo.delete(&created.id);
                return Err(storage_err(err));
            }
        }
        for (slot, value) in &plan.secrets {
            if let Err(err) = self.secret_store.store(&created.id, *slot, value) {
                let _ = self.secret_store.delete_all(&created.id);
                let _ = repo.delete(&created.id);
                return Err(StoreError::Storage {
                    detail: format!("store {}: {err}", slot.wire_name()),
                });
            }
        }
        if let Some(refresh) = refresh_for_sync {
            host_core::credential_sync::emit_credential_set(
                &self.writer,
                &shared,
                &self.plugin_manager,
                &created.id,
                SecretSlot::RefreshToken,
                &refresh,
            );
        }
        if let Err(err) = self.registry.register(&created) {
            let _ = self.secret_store.delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(StoreError::Storage {
                detail: format!("adapter registration failed: {err}"),
            });
        }
        append_account_event(
            &self.writer,
            &self.plugin_manager,
            created.adapter_kind.as_str(),
            SyncEvent::AccountCreated(account_payload(&created)),
        );
        to_json(&created)
    }
}

/// Helpers the exported surface uses internally.
///
/// A separate, NON-exported `impl` block on purpose: everything in the
/// `#[uniffi::export]` block above becomes part of the mobile API, and these
/// take types (a plugin manifest's schema, a resolved OAuth client) that have
/// no business crossing the FFI boundary — the client in particular holds a
/// secret.
impl Host {
    /// Follow an event that moved to another calendar, and tell the other
    /// devices. Best-effort, like the delete twin.
    fn relocate_event_grouping(
        &self,
        old_calendar_id: &str,
        old_event_id: &str,
        new_calendar_id: &str,
        new_event_id: &str,
    ) {
        let shared = self.db.shared();
        match EventGroupsRepo::new(&shared).relocate(
            old_calendar_id,
            old_event_id,
            new_calendar_id,
            new_event_id,
        ) {
            Ok(Some(moved)) => {
                self.emit_event_group(&moved.group);
                // The marks moved with the member; they have to travel too.
                self.emit_declines(&moved.carried);
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(?err, "could not follow the moved event's grouping"),
        }
    }

    /// Take a deleted calendar's events out of their groups, and tell the
    /// other devices.
    fn forget_calendar_groupings(&self, calendar_id: &str) {
        let shared = self.db.shared();
        match EventGroupsRepo::new(&shared).forget_calendar(calendar_id) {
            Ok(groups) => {
                for group in &groups {
                    self.emit_event_group(group);
                }
            }
            Err(err) => tracing::warn!(?err, "could not clear the calendar's groupings"),
        }
    }

    /// Take a deleted event out of whatever group it was in, and tell the
    /// other devices.
    ///
    /// A deleted event cannot go on meaning the same appointment as anything,
    /// and a membership row pointing at nothing is worse than none: the group
    /// still counts it and still names it. Passing the id through as-is is
    /// deliberate — memberships store the SERIES master id, so deleting a
    /// single occurrence finds no row and correctly changes nothing.
    ///
    /// Best-effort by design: the event IS deleted by the time this runs, and
    /// failing the whole call over the bookkeeping beside it would report a
    /// delete that actually happened as a failure.
    fn forget_event_grouping(&self, calendar_id: &str, event_id: &str) {
        let shared = self.db.shared();
        // Bookkeeping: a deleted event has not said that it is a different
        // appointment from anything — see `Removal`.
        match EventGroupsRepo::new(&shared).ungroup(calendar_id, event_id, Removal::Bookkeeping) {
            Ok(Some(Ungrouped::Remains { group, .. })) => self.emit_event_group(&group),
            Ok(Some(Ungrouped::Dissolved { group_id, .. })) => {
                self.writer
                    .append(SyncEvent::EventGroupDissolved(IdPayload { id: group_id }));
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(?err, "could not clear the deleted event's grouping")
            }
        }
    }

    /// Tell the other devices which pairs are not one appointment.
    ///
    /// Taking a group apart writes these as well as removing the membership,
    /// and both halves have to travel: a device that hears only "the group is
    /// gone" would form it again from the same evidence.
    fn emit_declines(&self, declines: &[cal_core::SuggestionDecline]) {
        for decline in declines {
            if let Ok(fields) = serde_json::to_value(decline) {
                self.writer
                    .append(SyncEvent::EventGroupSuggestionDeclined(EventPayload {
                        // The pair IS the identity; there is no id to mint.
                        id: format!(
                            "{} {} {} {}",
                            decline.calendar_a,
                            decline.event_a,
                            decline.calendar_b,
                            decline.event_b
                        ),
                        fields,
                    }));
            }
        }
    }

    /// A group change travels as the WHOLE membership — see `SyncEvent`'s own
    /// note on why a diff would let two devices interleave into a set neither
    /// of them meant.
    fn emit_event_group(&self, group: &cal_core::EventGroup) {
        if let Ok(fields) = serde_json::to_value(group) {
            self.writer
                .append(SyncEvent::EventGroupUpdated(EventPayload {
                    id: group.id.clone(),
                    fields,
                }));
        }
    }

    /// Read an event by id for the meeting commands.
    ///
    /// The same routing `get_event_by_id_json` uses — local rows straight from
    /// the store, external ones out of the SWR cache — so a recurring master
    /// resolves the same way here as it does everywhere else.
    fn read_event_for_meeting(
        &self,
        event_id: &str,
        calendar_id: &str,
    ) -> Result<Option<Event>, StoreError> {
        let json =
            self.get_event_by_id_json(event_id.to_string(), Some(calendar_id.to_string()))?;
        if json.trim() == "null" {
            return Ok(None);
        }
        let event: Event = from_json("event", &json)?;
        Ok(Some(event))
    }

    /// The string catalogue of the plugin backing `account_id`, if it ships one.
    ///
    /// `None` is an ordinary answer: a plugin with no catalogue renders its
    /// verbatim labels, which is what a third-party adapter with no
    /// translations does by design. Mirrors the desktop `adapter_catalogue`.
    fn adapter_catalogue(&self, account_id: &str) -> Option<plugin_core::StringCatalogue> {
        let shared = self.db.shared();
        let account = host_core::accounts::AccountsRepo::new(&shared)
            .get(account_id)
            .ok()
            .flatten()?;
        let plugin = self
            .plugin_manager
            .plugin_for_adapter_kind(account.adapter_kind.as_str())?;
        let catalogue = plugin.manifest.strings.clone();
        (!catalogue.is_empty()).then_some(catalogue)
    }

    /// Whether the calendar holding an event can invite its attendees itself —
    /// send them an invitation server-side and collect their replies as RSVPs.
    ///
    /// Read from the cached calendar listing, which the calendar screens have
    /// populated long before anyone opens an event editor. An unknown or
    /// uncached id degrades to `false`: the worst that follows is one
    /// invitation too many, whereas the other direction is a meeting nobody was
    /// told about. Mirrors the desktop `calendar_can_invite`.
    fn calendar_can_invite(&self, calendar_id: &str) -> bool {
        let Some(account) = self.registry.account_for_calendar(calendar_id) else {
            return false;
        };
        if account == LOCAL_ID {
            return false;
        }
        self.cache
            .read_calendars(&account)
            .ok()
            .into_iter()
            .flatten()
            .find(|c| c.id == calendar_id)
            .is_some_and(|c| c.supports_scheduling)
    }

    /// Save an event edited by the meeting commands, through the same write
    /// path every other edit takes.
    fn save_event_for_meeting(&self, event: Event) -> Result<Event, StoreError> {
        let json = self.update_event_json(to_json(&event)?, None)?;
        from_json("event", &json)
    }

    /// The plugin id + account schema for an adapter kind, or a typed error.
    /// The argument object a plugin's auth phase receives.
    ///
    /// Starts from the values the account or the form actually holds, so
    /// EVERYTHING the adapter declared travels — Microsoft's tenant, Webex's
    /// site — and overwrites only the client, which the host resolved and the
    /// caller may not know (a built-in-posture account has no client id of its
    /// own). The hand-written objects this replaces listed two or three fields
    /// each and silently dropped the rest, which is invisible right up until a
    /// single-tenant Azure registration is rejected for being asked at
    /// `/common`.
    fn auth_args(
        oauth: &plugin_core::account_schema::AccountOauth,
        values: &serde_json::Map<String, serde_json::Value>,
        client: &host_core::account_setup::OauthClient,
        phase: &str,
        extra: &[(&str, serde_json::Value)],
    ) -> serde_json::Value {
        let mut args = values.clone();
        // Under the name the SCHEMA gave the field, and under the protocol's
        // own, so an adapter that calls it something else is still understood.
        for key in [oauth.client_id_field.as_str(), "client_id"] {
            args.insert(
                key.to_string(),
                serde_json::Value::String(client.id.clone()),
            );
        }
        let secret = client
            .secret
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null);
        if let Some(field) = oauth.client_secret_field.as_deref() {
            args.insert(field.to_string(), secret.clone());
        }
        args.insert("client_secret".to_string(), secret);
        args.insert(
            "phase".to_string(),
            serde_json::Value::String(phase.to_string()),
        );
        args.insert(
            "redirect_uri".to_string(),
            serde_json::Value::String(oauth.app_redirect_uri.clone()),
        );
        for (key, value) in extra {
            args.insert((*key).to_string(), value.clone());
        }
        serde_json::Value::Object(args)
    }

    /// The values a RECONNECT signs in with.
    ///
    /// A reconnect needs no form: the account already holds everything the
    /// original sign-in settled. The non-secret half is its `config_json`; the
    /// client secret, if this account brought its own registration, is in the
    /// keychain. An account on the build's own registration has neither — and
    /// that is the point, because `choose_oauth_client` reads the empty pair as
    /// "use whatever this build carries", which is exactly what it did the first
    /// time.
    fn reconnect_values(
        &self,
        account: &host_core::accounts::Account,
        oauth: &plugin_core::account_schema::AccountOauth,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut values: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&account.config_json).unwrap_or_default();
        if let Some(field) = oauth.client_secret_field.as_deref() {
            if let Ok(secret) = self
                .secret_store
                .retrieve(&account.id, SecretSlot::OauthClientSecret)
            {
                values.insert(field.to_string(), serde_json::Value::String(secret));
            }
        }
        values
    }

    /// Look an account up and resolve the schema its adapter declares, refusing
    /// early when that adapter does not sign in through a provider.
    fn oauth_account(
        &self,
        account_id: &str,
    ) -> Result<
        (
            host_core::accounts::Account,
            String,
            plugin_core::account_schema::AccountSchema,
        ),
        StoreError,
    > {
        let shared = self.db.shared();
        let account = AccountsRepo::new(&shared)
            .get(account_id)
            .map_err(acc_err)?
            .ok_or(StoreError::NotFound)?;
        let (plugin_id, schema) = self.schema_for(account.adapter_kind.as_str())?;
        if schema.oauth.is_none() {
            return Err(StoreError::InvalidField {
                field: "account_id".to_string(),
                detail: format!(
                    "account kind {} does not sign in through a provider",
                    account.adapter_kind.as_str()
                ),
            });
        }
        Ok((account, plugin_id, schema))
    }

    fn schema_for(
        &self,
        adapter_kind: &str,
    ) -> Result<(String, plugin_core::account_schema::AccountSchema), StoreError> {
        let plugin = self
            .plugin_manager
            .plugin_for_adapter_kind(adapter_kind)
            .ok_or_else(|| StoreError::InvalidField {
                field: "adapter_kind".to_string(),
                detail: format!("no plugin serves adapter kind `{adapter_kind}`"),
            })?;
        let schema = plugin
            .manifest
            .account
            .clone()
            .ok_or_else(|| StoreError::InvalidField {
                field: "adapter_kind".to_string(),
                detail: "this adapter declares no account schema".to_string(),
            })?;
        Ok((plugin.manifest.id.clone(), schema))
    }

    /// Resolve the OAuth client from the form's values + this build's posture.
    fn oauth_client_for(
        &self,
        oauth: &plugin_core::account_schema::AccountOauth,
        values: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<host_core::account_setup::OauthClient, StoreError> {
        host_core::account_setup::choose_oauth_client(
            oauth,
            supplied_value(values, &oauth.client_id_field).as_deref(),
            oauth
                .client_secret_field
                .as_deref()
                .and_then(|k| supplied_value(values, k))
                .as_deref(),
        )
        .map(|choice| choice.client)
        .map_err(setup_err)
    }
}

/// Request body for [`Host::connect_account_json`] — the form's values plus,
/// for an adapter that signs in via OAuth, the redirect's outcome.
///
/// The OAuth fields are absent for an adapter with no OAuth block, which is why
/// they all default: one request shape serves both, so the mobile UI has one
/// call to make either way.
#[derive(serde::Deserialize)]
struct ConnectAccountRequest {
    adapter_kind: String,
    display_name: String,
    /// When set, EDIT this existing account instead of creating one — the
    /// Accounts screen's edit form. Optional and absent on every create, so
    /// old callers are untouched; carried in the JSON body precisely so the
    /// UniFFI method signature (and thus the committed Kotlin bindings) stays
    /// unchanged.
    #[serde(default)]
    account_id: Option<String>,
    /// Keyed by the schema's field keys.
    #[serde(default)]
    values: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    code: String,
    #[serde(default)]
    pkce_verifier: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    returned_state: String,
}

/// The form's own values, for the calls that only need those.
#[derive(serde::Deserialize)]
struct SchemaFormRequest {
    adapter_kind: String,
    /// Set by the EDIT form: a secret field left blank then probes with the
    /// credential stored for this account — see the desktop twin.
    #[serde(default)]
    account_id: Option<String>,
    /// Keyed by the schema's field keys.
    #[serde(default)]
    values: serde_json::Map<String, serde_json::Value>,
}

/// …plus which declared action to run.
#[derive(serde::Deserialize)]
struct AccountActionRequest {
    adapter_kind: String,
    action_key: String,
    #[serde(default)]
    values: serde_json::Map<String, serde_json::Value>,
}

/// A form value as a trimmed string, or `None` when absent or not a string.
fn supplied_value(
    values: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    values
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Map an account-setup failure onto the FFI error, keeping the three meanings
/// apart: something the user can fix, a row that no longer makes sense, and a
/// credential store that would not answer.
fn setup_err(err: host_core::account_setup::AccountSetupError) -> StoreError {
    use host_core::account_setup::AccountSetupError as E;
    match err {
        E::InvalidInput(detail) => StoreError::InvalidField {
            field: "account".to_string(),
            detail,
        },
        E::Config(detail) => StoreError::InvalidField {
            field: "config_json".to_string(),
            detail,
        },
        E::Secret(detail) => StoreError::Storage { detail },
    }
}

/// Request body for [`Host::attach_meeting_json`] and
/// [`Host::detach_meeting_json`]. `account_id` is ignored on detach — the
/// binding already names the account that minted the meeting.
#[derive(serde::Deserialize)]
struct AttachMeetingRequest {
    event_id: String,
    calendar_id: String,
    #[serde(default)]
    account_id: String,
    /// Link the account's permanent room instead of minting a meeting.
    #[serde(default)]
    use_personal_room: bool,
    /// Which language the join block is written in. Per meeting, and not
    /// necessarily the app's — the block is frozen into the event the moment it
    /// is written, and lands in other people's calendars where nothing can
    /// re-render it. Absent falls back to English.
    #[serde(default)]
    invitation_lang: Option<String>,
}

/// Request body for [`Host::adopt_meeting_json`].
#[derive(serde::Deserialize)]
struct AdoptMeetingRequest {
    event_id: String,
    account_id: String,
    meeting_id: String,
    join_url: String,
}

fn meetings_err(err: host_core::meetings::MeetingsError) -> StoreError {
    StoreError::Storage {
        detail: err.to_string(),
    }
}

fn vc_err(err: vc_core::VcError) -> StoreError {
    match err {
        vc_core::VcError::Authentication(detail) => StoreError::Auth { detail },
        vc_core::VcError::Forbidden(detail) => StoreError::Forbidden { detail },
        vc_core::VcError::NotFound(_) => StoreError::NotFound,
        vc_core::VcError::InvalidInput(detail) => StoreError::InvalidField {
            field: "meeting".to_string(),
            detail,
        },
        other => StoreError::Storage {
            detail: other.to_string(),
        },
    }
}

/// The bits of the plugin's token response the host needs to persist.
///
/// Only the refresh token is read: the one remaining caller is the SYNC
/// adapter path, which re-mints access tokens from it. The access token stays
/// declared so a payload carrying one still deserialises, and named with a
/// leading underscore so it is clear that is deliberate rather than forgotten.
#[derive(serde::Deserialize)]
struct OAuthTokenJson {
    #[serde(rename = "access_token")]
    _access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Request body for [`Host::complete_sync_oauth_json`] — the token-exchange
/// inputs forwarded to the sync plugin's `phase:"exchange"`. No account fields
/// (sync OAuth stores only the refresh token in the adapter's keychain slot).
/// What a re-sign-in sends back. No client fields: the account already holds
/// them, and asking the caller to resend a client secret it had to read out of
/// the keychain first is a round trip that can only go wrong.
#[derive(serde::Deserialize)]
struct AccountReconnectRequest {
    code: String,
    pkce_verifier: String,
    state: String,
    returned_state: String,
    /// The bring-your-own OAuth client secret, typed on a device that does not
    /// hold it. `serde(default)`, so every older caller stays valid — no
    /// signature and therefore no binding changes.
    #[serde(default)]
    client_secret: Option<String>,
}

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

/// Parse a full-round trigger wire string → `SyncTrigger`. Unknown ⇒ `Manual`
/// (the user-initiated default; the Settings "Sync now" button sends "manual").
fn parse_sync_trigger(s: &str) -> SyncTrigger {
    match s {
        "app_start" => SyncTrigger::AppStart,
        "periodic" => SyncTrigger::Periodic,
        "background" => SyncTrigger::Background,
        "kick" => SyncTrigger::Kick,
        "app_exit" => SyncTrigger::AppExit,
        _ => SyncTrigger::Manual,
    }
}

/// Parse a push-only trigger wire string → `SyncTrigger`. Unknown ⇒ `Kick` (the
/// debounced-after-mutation default, the most common push path on mobile).
fn parse_push_trigger(s: &str) -> SyncTrigger {
    match s {
        "app_exit" => SyncTrigger::AppExit,
        "manual" => SyncTrigger::Manual,
        "periodic" => SyncTrigger::Periodic,
        _ => SyncTrigger::Kick,
    }
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

/// Compare a freshly-observed fingerprint against this device's pin store and
/// build the `HostKeyPreview` wire shape.
///
/// Split out so the account-based probe reaches the SAME three-way verdict as
/// the form-based one. A second copy of this comparison is the one place a bug
/// would be invisible: it would classify a CHANGED key as first use, and the
/// user would be shown the benign prompt for what is the alarm case.
fn classify_host_key(
    shared: &SharedConn,
    host_port: String,
    fingerprint: String,
) -> Result<serde_json::Value, StoreError> {
    // `try_peek`, not `peek`: this is the one place the pin is COMPARED. A read
    // failure folded into `None` would classify a host key that CHANGED as
    // first use — the user sees the benign TOFU prompt instead of the §19.5
    // alarm, confirms, and `trust_sftp_host_key` writes the presented
    // fingerprint over a pin we could not read. Refuse the preview instead;
    // nothing is pinned and nothing is connected until the user retries.
    let stored = UserPrefsHostKeyVerifier::new(shared.clone())
        .try_peek(&host_port)
        .map_err(|err| StoreError::Storage {
            detail: format!("read the pinned host key for {host_port}: {err}"),
        })?;
    let status = match stored {
        None => serde_json::json!({ "kind": "new" }),
        Some(ref s) if *s == fingerprint => serde_json::json!({ "kind": "unchanged" }),
        Some(s) => serde_json::json!({ "kind": "changed", "stored": s }),
    };
    Ok(serde_json::json!({
        "host_port": host_port,
        "fingerprint": fingerprint,
        "status": status,
    }))
}

/// Why an account could not be opened as this device's sync target, as the
/// `StoreError` this boundary speaks.
///
/// Every variant carries [`host_core::sync_target::Unbuildable`]'s own text: it
/// names the field or repeats the plugin's complaint — "no password stored for
/// the sync target", "sync plugin refused: …" — which is more than a
/// re-worded stand-in could say. The VARIANT is chosen for what the user has to
/// do about it, which is the only distinction this boundary can carry: a
/// credential to re-enter is `Auth`, a row that is gone is `NotFound`,
/// everything else is a rejected field.
fn unbuildable_err(err: host_core::sync_target::Unbuildable) -> StoreError {
    use host_core::sync_target::Unbuildable as U;
    match &err {
        // Both are "this device is missing something only it can supply". The
        // host key is not a credential, but the repair is the same shape — a
        // gesture on this device — and the sync screen offers it through
        // `preview_sync_account_host_key_json` rather than by reading this.
        U::MissingCredential { .. } | U::HostKeyNotTrusted { .. } => StoreError::Auth {
            detail: err.to_string(),
        },
        U::AccountMissing { .. } => StoreError::NotFound,
        U::NotConfigured | U::Incomplete { .. } | U::Invalid { .. } | U::PluginRefused { .. } => {
            StoreError::InvalidField {
                field: "sync_target".to_string(),
                detail: err.to_string(),
            }
        }
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
    fn logging_init_is_idempotent_across_hosts_and_set_level_works() {
        // Two Host::open in one binary must NOT double-install the global
        // subscriber (the Once guard). The level facade works on the cached
        // handle regardless of which host set it; the default reads back "info".
        let (_d1, h1, _k1) = open_host();
        let (_d2, h2, _k2) = open_host();
        assert_eq!(h1.get_log_level().unwrap(), "info");
        h2.set_log_level("debug".into()).unwrap();
        assert_eq!(h2.get_log_level().unwrap(), "debug");
        // A bad level is rejected, not stored.
        assert!(h2.set_log_level("loud".into()).is_err());
        // The logs dir resolved (logging initialised), and a recent-lines read
        // doesn't error even with no content yet.
        assert!(h1.logs_dir_path().unwrap().contains("logs"));
        let _ = h1.get_recent_logs(Some(10)).unwrap();
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
    fn editing_an_account_updates_config_and_keeps_a_blank_secret() {
        let (_dir, host, kc) = open_host();
        let created = host
            .create_account_json(
                r#"{
                    "adapter_kind": "caldav",
                    "display_name": "Work CalDAV",
                    "config_json": "{\"server_url\":\"https://old.example.invalid/\",\"username\":\"alice\",\"auth_kind\":\"basic\"}",
                    "secret": "hunter2"
                }"#
                .to_string(),
            )
            .unwrap();
        let id: serde_json::Value = serde_json::from_str(&created).unwrap();
        let account_id = id["id"].as_str().unwrap().to_string();

        // EDIT: new URL + name, secret field left BLANK — the stored password
        // must survive byte for byte, and the undeclared `auth_kind` key must
        // carry over (the form owns only the schema's declared fields).
        let edit = format!(
            r#"{{
                "adapter_kind": "caldav",
                "display_name": "Renamed CalDAV",
                "account_id": "{account_id}",
                "values": {{
                    "server_url": "https://new.example.invalid/",
                    "username": "alice"
                }}
            }}"#
        );
        let updated = host.connect_account_json(edit).unwrap();
        let row: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(row["display_name"], "Renamed CalDAV");
        let config: serde_json::Value =
            serde_json::from_str(row["config_json"].as_str().unwrap()).unwrap();
        assert_eq!(config["server_url"], "https://new.example.invalid/");
        assert_eq!(
            config["auth_kind"], "basic",
            "undeclared config keys must carry over"
        );
        {
            let stored = kc.map.lock().unwrap();
            assert!(
                stored
                    .iter()
                    .any(|((_, slot), v)| slot == "password" && v == "hunter2"),
                "a blank secret field must keep the stored credential"
            );
        }

        // EDIT with a NEW secret replaces the stored one.
        let edit2 = format!(
            r#"{{
                "adapter_kind": "caldav",
                "display_name": "Renamed CalDAV",
                "account_id": "{account_id}",
                "values": {{
                    "server_url": "https://new.example.invalid/",
                    "username": "alice",
                    "secret": "hunter3"
                }}
            }}"#
        );
        host.connect_account_json(edit2).unwrap();
        let stored = kc.map.lock().unwrap();
        assert!(stored
            .iter()
            .any(|((_, slot), v)| slot == "password" && v == "hunter3"));
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
    fn rename_account_trims_and_persists_and_rejects_empty() {
        let (_dir, host, _kc) = open_host();
        let req = r#"{
            "adapter_kind": "caldav",
            "display_name": "Old Name",
            "config_json": "{\"server_url\":\"https://dav.example.invalid/\",\"username\":\"a\",\"auth_kind\":\"basic\"}",
            "secret": "pw"
        }"#;
        let created = host.create_account_json(req.to_string()).unwrap();
        let id: serde_json::Value = serde_json::from_str(&created).unwrap();
        let account_id = id["id"].as_str().unwrap().to_string();

        let renamed = host
            .rename_account_json(account_id.clone(), "  New Name  ".to_string())
            .unwrap();
        assert!(
            renamed.contains("\"display_name\":\"New Name\""),
            "name is trimmed + persisted",
        );
        assert!(host.accounts_json().unwrap().contains("New Name"));

        // Empty name → InvalidField.
        assert!(matches!(
            host.rename_account_json(account_id, "   ".to_string()).unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "name"
        ));
    }

    #[test]
    fn missing_credentials_flags_a_lost_secret_and_set_account_secret_restores_it() {
        let (_dir, host, kc) = open_host();
        let req = r#"{
            "adapter_kind": "caldav",
            "display_name": "Work",
            "config_json": "{\"server_url\":\"https://dav.example.invalid/\",\"username\":\"a\",\"auth_kind\":\"basic\"}",
            "secret": "pw"
        }"#;
        let account_id = serde_json::from_str::<serde_json::Value>(
            &host.create_account_json(req.to_string()).unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // With its password present, the account is NOT flagged.
        let missing: serde_json::Value =
            serde_json::from_str(&host.list_accounts_missing_credentials_json().unwrap()).unwrap();
        assert!(missing.as_array().unwrap().is_empty());

        // Simulate the keychain losing the secret (token expiry / a row synced
        // from another device without its device-local secret).
        kc.map
            .lock()
            .unwrap()
            .remove(&(account_id.clone(), "password".to_string()));
        let missing: serde_json::Value =
            serde_json::from_str(&host.list_accounts_missing_credentials_json().unwrap()).unwrap();
        assert!(missing
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == serde_json::json!(account_id)));

        // Re-enter the credential → stored under the password slot + no longer flagged.
        host.set_account_secret(account_id.clone(), "newpw".to_string())
            .unwrap();
        assert!(kc
            .map
            .lock()
            .unwrap()
            .iter()
            .any(|((acc, slot), v)| acc == &account_id && slot == "password" && v == "newpw"));
        let missing: serde_json::Value =
            serde_json::from_str(&host.list_accounts_missing_credentials_json().unwrap()).unwrap();
        assert!(missing.as_array().unwrap().is_empty());
    }

    #[test]
    fn set_account_secret_rejects_an_unknown_account() {
        let (_dir, host, _kc) = open_host();
        assert!(matches!(
            host.set_account_secret("nope".to_string(), "x".to_string())
                .unwrap_err(),
            StoreError::NotFound
        ));
    }

    #[test]
    fn a_reconnect_refuses_an_unknown_account_and_one_that_types_its_credential() {
        let (_dir, host, _kc) = open_host();
        let req = r#"{"code":"c","pkce_verifier":"v","state":"s","returned_state":"s"}"#;
        // Unknown account, refused before any exchange is attempted.
        assert!(matches!(
            host.complete_account_reconnect_json("nope".to_string(), req.to_string())
                .unwrap_err(),
            StoreError::NotFound
        ));
        // A CalDAV account signs in with a password, so the sign-in path must
        // send it back to set_account_secret rather than open a browser. This
        // is now decided by the adapter's schema, not by its name.
        let caldav = r#"{
            "adapter_kind": "caldav",
            "display_name": "Work",
            "config_json": "{\"server_url\":\"https://dav.example.invalid/\",\"username\":\"a\",\"auth_kind\":\"basic\"}",
            "secret": "pw"
        }"#;
        let id = serde_json::from_str::<serde_json::Value>(
            &host.create_account_json(caldav.to_string()).unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(matches!(
            host.complete_account_reconnect_json(id.clone(), req.to_string())
                .unwrap_err(),
            StoreError::InvalidField { .. }
        ));
        // And the begin half agrees, so the UI cannot open a session it would
        // then be unable to finish.
        assert!(matches!(
            host.begin_account_reconnect_json(id).unwrap_err(),
            StoreError::InvalidField { .. }
        ));
    }

    /// Seed a local contacts list and return its id. Migration 0007 no
    /// longer seeds a default address book, so contacts tests create one
    /// explicitly (the user does this from the Sidebar on a real install).
    fn seed_contact_list(host: &Host) -> String {
        let created: serde_json::Value = serde_json::from_str(
            &host
                .create_contact_list_json("Contacts".to_string())
                .unwrap(),
        )
        .unwrap();
        created["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn search_contacts_finds_a_local_contact() {
        let (_dir, host, _kc) = open_host();
        let list_id = seed_contact_list(&host);
        let contact = r#"{
            "display_name": "Alice Example",
            "given_name": "Alice",
            "family_name": "Example",
            "organization": null,
            "emails": ["alice@example.com"],
            "phone_numbers": [],
            "birthday": null,
            "notes": null,
            "addresses": [],
            "members": null,
            "photo": null
        }"#;
        host.create_contact_json(list_id, contact.to_string())
            .unwrap();

        let results: serde_json::Value =
            serde_json::from_str(&host.search_contacts_json("Alice".to_string()).unwrap()).unwrap();
        assert!(
            results
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["display_name"] == "Alice Example"),
            "the local contact is found by name",
        );
    }

    #[test]
    fn contact_photo_round_trips_set_get_and_delete() {
        let (_dir, host, _kc) = open_host();
        let list_id = seed_contact_list(&host);
        let contact = r#"{
            "display_name": "Bob Example",
            "given_name": "Bob",
            "family_name": "Example",
            "organization": null,
            "emails": [],
            "phone_numbers": [],
            "birthday": null,
            "notes": null,
            "addresses": [],
            "members": null,
            "photo": null
        }"#;
        let created: serde_json::Value = serde_json::from_str(
            &host
                .create_contact_json(list_id.clone(), contact.to_string())
                .unwrap(),
        )
        .unwrap();
        let contact_id = created["id"].as_str().unwrap().to_string();
        // No photo initially.
        assert_eq!(created["has_photo"], serde_json::json!(false));
        assert_eq!(
            host.get_contact_photo_json(contact_id.clone(), Some(list_id.clone()))
                .unwrap()
                .trim(),
            "null"
        );

        // Set a photo (data is base64 on the wire: "aGVsbG8=" = b"hello").
        let photo = r#"{"content_type":"image/png","data":"aGVsbG8="}"#;
        host.set_contact_photo_json(contact_id.clone(), Some(list_id.clone()), photo.to_string())
            .unwrap();
        let got: serde_json::Value = serde_json::from_str(
            &host
                .get_contact_photo_json(contact_id.clone(), Some(list_id.clone()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got["content_type"], "image/png");
        assert_eq!(got["data"], "aGVsbG8=", "the bytes round-trip as base64");

        // Delete it → back to null.
        host.delete_contact_photo(contact_id.clone(), Some(list_id.clone()))
            .unwrap();
        assert_eq!(
            host.get_contact_photo_json(contact_id, Some(list_id))
                .unwrap()
                .trim(),
            "null"
        );
    }

    #[test]
    fn sync_now_records_a_sync_log_row_and_clear_empties_it() {
        let remote = tempfile::tempdir().unwrap();
        let cfg = format!(
            r#"{{"kind":"local","path":{}}}"#,
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap()
        );
        let dir = tempfile::tempdir().unwrap();
        let host = open_named(&dir, "log");
        host.configure_sync_adapter_json(cfg).unwrap();

        // Empty to start.
        let empty: serde_json::Value =
            serde_json::from_str(&host.list_sync_log_json(50).unwrap()).unwrap();
        assert!(empty.as_array().unwrap().is_empty());

        // A manual round records exactly one row, tagged + timed.
        host.sync_now_json("manual".to_string()).unwrap();
        let entries: serde_json::Value =
            serde_json::from_str(&host.list_sync_log_json(50).unwrap()).unwrap();
        let arr = entries.as_array().unwrap();
        assert_eq!(
            arr.len(),
            1,
            "the manual round recorded one row; got {entries}"
        );
        assert_eq!(arr[0]["trigger"], "manual");
        assert_eq!(arr[0]["success"], true);
        assert!(arr[0]["duration_ms"].is_number());

        // Clearing empties it.
        host.clear_sync_log().unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&host.list_sync_log_json(50).unwrap()).unwrap();
        assert!(after.as_array().unwrap().is_empty());
    }

    #[test]
    fn add_event_exdate_appends_an_exception_to_a_local_series() {
        let (_dir, host, _kc) = open_host();
        let cal: serde_json::Value = serde_json::from_str(
            &host
                .create_calendar_json(serde_json::json!({"name": "Cal"}).to_string())
                .unwrap(),
        )
        .unwrap();
        let cal_id = cal["id"].as_str().unwrap().to_string();
        let new_event = serde_json::json!({
            "calendar_id": cal_id,
            "title": "Standup",
            "description": null,
            "location": null,
            "start": "2026-06-01T09:00:00Z",
            "end": "2026-06-01T09:15:00Z",
            "all_day": false,
            "recurrence": { "rrule": "FREQ=DAILY", "exceptions": [] },
            "color_label": null,
            "reminders": [],
            "sound": null,
            "attendees": []
        });
        let created: serde_json::Value =
            serde_json::from_str(&host.create_event_json(new_event.to_string()).unwrap()).unwrap();
        let id = created["id"].as_str().unwrap().to_string();

        host.add_event_exdate_json(
            id.clone(),
            "2026-06-02T09:00:00Z".to_string(),
            Some(cal_id),
            false,
        )
        .unwrap();

        let refreshed: serde_json::Value =
            serde_json::from_str(&host.get_event_by_id_json(id, None).unwrap()).unwrap();
        assert_eq!(
            refreshed["recurrence"]["exceptions"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "the excluded occurrence is appended to the master's exceptions",
        );

        // A non-RFC-3339 occurrence is rejected.
        assert!(matches!(
            host.add_event_exdate_json("x".to_string(), "nope".to_string(), None, false)
                .unwrap_err(),
            StoreError::InvalidField { ref field, .. } if field == "occurrence"
        ));
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
            serde_json::from_str(&host.get_event_by_id_json(id.clone(), None).unwrap()).unwrap();
        assert_eq!(one["id"], serde_json::json!(id));
    }

    #[test]
    fn update_event_changes_title_and_persists() {
        let (_dir, host, _kc) = open_host();
        let cal = make_calendar(&host);
        let created = host.create_event_json(new_event_json(&cal, "Old")).unwrap();
        let mut event: serde_json::Value = serde_json::from_str(&created).unwrap();
        event["title"] = serde_json::json!("New");
        let updated = host.update_event_json(event.to_string(), None).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&updated).unwrap()["title"],
            "New"
        );
        let id = event["id"].as_str().unwrap().to_string();
        let reread: serde_json::Value =
            serde_json::from_str(&host.get_event_by_id_json(id, None).unwrap()).unwrap();
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

        // A local→local move passes the source as previous_calendar_id, taking
        // the move-detection branch — which the desktop treats as a single SQL
        // UPDATE routed by event.calendar_id.
        event["calendar_id"] = serde_json::json!(cal_b);
        host.update_event_json(event.to_string(), Some(cal_a.clone()))
            .unwrap();

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
    fn passing_previous_equal_to_current_is_an_in_place_update_not_a_move() {
        let (_dir, host, _kc) = open_host();
        let cal = make_calendar(&host);
        let created = host.create_event_json(new_event_json(&cal, "Old")).unwrap();
        let mut event: serde_json::Value = serde_json::from_str(&created).unwrap();
        let id = event["id"].as_str().unwrap().to_string();
        // previous_calendar_id == event.calendar_id → not a move; the title edit
        // applies in place against the same row (no create-then-delete).
        event["title"] = serde_json::json!("Renamed");
        host.update_event_json(event.to_string(), Some(cal.clone()))
            .unwrap();
        let reread: serde_json::Value =
            serde_json::from_str(&host.get_event_by_id_json(id, None).unwrap()).unwrap();
        assert_eq!(reread["title"], "Renamed");
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
        assert_eq!(host.get_event_by_id_json(id, None).unwrap().trim(), "null");
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
        let err = host.update_event_json(event.to_string(), None).unwrap_err();
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
    /// The sync screen's whole verb, end to end — on the account every device
    /// already has.
    ///
    /// Folder sync folded into the built-in store, so "mirror my data into this
    /// folder" is a property of the `local` account rather than a second one
    /// beside it. That makes this the smallest end-to-end case there is: no
    /// network, no credential, and the row is not even created here because it
    /// exists from bootstrap. What is written is the folder — device-local, so
    /// it lands in this device's preferences rather than in a row that travels
    /// — and the pointer.
    #[test]
    fn selecting_the_built_in_store_makes_it_the_sync_target() {
        let remote = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let host = open_named(&dir, "select");

        let before = host.accounts_json().unwrap();

        // The connect path, with the built-in store's own declared field. No
        // account is created: `sync_target::connect` finds the implicit row.
        host.adopt_local_dataset_values_json(
            serde_json::json!({
                "adapter_kind": "local",
                "values": { "remote_root": remote.path().to_string_lossy() },
            })
            .to_string(),
            Some("This device".to_string()),
            None,
        )
        .unwrap();

        let after = host.accounts_json().unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<serde_json::Value>>(&before)
                .unwrap()
                .len(),
            serde_json::from_str::<Vec<serde_json::Value>>(&after)
                .unwrap()
                .len(),
            "choosing the built-in store as the storage must not mint a second \
             account: it is the one that was already there",
        );

        let summary: serde_json::Value =
            serde_json::from_str(&host.get_sync_adapter_summary_json().unwrap()).unwrap();
        assert_eq!(summary["kind"], "local");
        assert_eq!(
            summary["account_id"],
            serde_json::Value::String("local".to_string())
        );

        // Probed AND activated, not merely written down: a round runs.
        host.sync_now_json("manual".to_string()).unwrap();

        // And selecting it again by id is the sync screen's own verb, which has
        // to reach the same row.
        host.select_sync_account("local".to_string()).unwrap();
        let again: serde_json::Value =
            serde_json::from_str(&host.get_sync_adapter_summary_json().unwrap()).unwrap();
        assert_eq!(again["account_id"], summary["account_id"]);
    }

    /// A refusal must leave this device syncing exactly where it did, which for
    /// a device that syncs nowhere means still nowhere — the pointer is written
    /// last, after the probe.
    #[test]
    fn selecting_an_account_that_is_not_there_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let host = open_named(&dir, "missing");
        assert!(matches!(
            host.select_sync_account("nope".to_string()),
            Err(StoreError::NotFound)
        ));
        assert!(host.select_sync_account("   ".to_string()).is_err());
        assert_eq!(host.get_sync_adapter_summary_json().unwrap(), "null");
    }

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
        // The writer appends events to a channel; a background drain task writes
        // them to the session file. A test that logs SEVERAL events (e.g.
        // CalendarCreated + EventCreated) must wait for ALL of them to reach disk
        // before pushing — not just the first byte. Returning early (the old
        // `bytes > 0` check) let a slow machine push a partial log, so the peer
        // missed the later events (flaky on CI windows/macos). Wait until the
        // total pending size is non-zero AND has stopped growing for a few
        // consecutive samples — i.e. the drain has caught up with the queue.
        let pending = dir.path().join("sync").join("log").join("pending");
        let total = || -> u64 {
            std::fs::read_dir(&pending)
                .map(|es| {
                    es.flatten()
                        .filter_map(|e| std::fs::metadata(e.path()).ok())
                        .map(|m| m.len())
                        .sum()
                })
                .unwrap_or(0)
        };
        let mut last = 0u64;
        let mut stable = 0u32;
        for _ in 0..200 {
            let bytes = total();
            if bytes > 0 && bytes == last {
                stable += 1;
                if stable >= 3 {
                    return;
                }
            } else {
                stable = 0;
            }
            last = bytes;
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
        host_a.sync_now_json("manual".to_string()).unwrap();
        host_b.sync_now_json("manual".to_string()).unwrap();

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
        host_b.sync_now_json("manual".to_string()).unwrap();
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
        host_a.sync_now_json("manual".to_string()).unwrap();
        host_b.sync_now_json("manual".to_string()).unwrap();

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
        host_b.sync_now_json("manual".to_string()).unwrap();
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
    fn compaction_does_not_flag_caught_up_peers_stale() {
        // §19.10 end-to-end regression. After one device compacts, a fully
        // caught-up PEER must keep syncing normally — the held-horizon
        // backstop must NOT mistake it for a device that fell behind the
        // snapshot. The reverted cursor-only backstop sent healthy peers to
        // the resume dialog on every round (snapshot_ts = now-1s sat a hair
        // above every foreign log a caught-up cursor could reach); this drives
        // a real foreign cursor against a real compactor snapshot_ts and
        // asserts NO StaleDevice. It's the integration coverage whose absence
        // let that bug ship.
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

        let task = r#"{"title":"t","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;

        // A writes a list + task; both round so B catches up to A.
        let list = host_a.create_task_list_json("Shared".to_string()).unwrap();
        let list_id = serde_json::from_str::<serde_json::Value>(&list).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        host_a
            .create_task_json(list_id.clone(), task.to_string())
            .unwrap();
        wait_for_pending(&dir_a);
        host_a.sync_now_json("manual".to_string()).unwrap();
        host_b.sync_now_json("manual".to_string()).unwrap();

        // B writes a task; both round so A catches up to B. Now BOTH hold
        // every log in the dataset (own-written + foreign-applied).
        host_b
            .create_task_json(list_id.clone(), task.to_string())
            .unwrap();
        wait_for_pending(&dir_b);
        host_b.sync_now_json("manual".to_string()).unwrap();
        host_a.sync_now_json("manual".to_string()).unwrap();

        // A compacts: snapshot_ts = max(own_a, cursor_a) = the dataset's real
        // content horizon, which both caught-up devices' held horizons reach.
        host_a.compact_now_json().unwrap();

        // The regression assertion: neither device is flagged stale on its
        // next round. A StaleDevice surfaces as `Err` (code "stale_device").
        let b_round = host_b.sync_now_json("manual".to_string());
        assert!(
            b_round.is_ok(),
            "a caught-up peer must not be flagged stale after a compaction; got {b_round:?}",
        );
        let a_round = host_a.sync_now_json("manual".to_string());
        assert!(
            a_round.is_ok(),
            "the compacting device must not flag itself stale; got {a_round:?}",
        );
        // No stale latch on B's status either.
        let status_b: serde_json::Value =
            serde_json::from_str(&host_b.sync_status_json().unwrap()).unwrap();
        assert_eq!(
            status_b["stale_device_since"],
            serde_json::Value::Null,
            "B must carry no stale latch after syncing past a compaction",
        );
    }

    #[test]
    fn a_stale_device_auto_resumes_without_a_prompt() {
        // §19.10 regression: a device that fell behind the GC horizon must
        // recover FULLY AUTOMATICALLY on its next round (re-pull the snapshot
        // inline) — never surfacing "device offline too long" / a failed round,
        // and never requiring a manual Fortfahren. We simulate the stale flag
        // the compactor would have set, then assert B's next sync_now
        // auto-resumes (Ok), leaves no stale latch, and still holds the data.
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

        let task = r#"{"title":"t","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let list = host_a.create_task_list_json("Shared".to_string()).unwrap();
        let list_id = serde_json::from_str::<serde_json::Value>(&list).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        host_a
            .create_task_json(list_id.clone(), task.to_string())
            .unwrap();
        wait_for_pending(&dir_a);
        host_a.sync_now_json("manual".to_string()).unwrap();
        host_b.sync_now_json("manual".to_string()).unwrap();

        // A compacts so a snapshot.json exists for the resume to pull.
        host_a.compact_now_json().unwrap();

        // Simulate the compactor flagging devices stale (B fell behind the GC
        // horizon): set stale=true on every device entry in meta.json.
        let meta_path = remote.path().join("meta.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        for (_id, dev) in meta["devices"].as_object_mut().unwrap() {
            dev["stale"] = serde_json::Value::Bool(true);
        }
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

        // B's next round must AUTO-RESUME — Ok, not StaleDevice.
        let b_round = host_b.sync_now_json("manual".to_string());
        assert!(
            b_round.is_ok(),
            "a stale device must auto-resume without a manual prompt; got {b_round:?}",
        );
        // No stale latch surfaced to the UI.
        let status_b: serde_json::Value =
            serde_json::from_str(&host_b.sync_status_json().unwrap()).unwrap();
        assert_eq!(
            status_b["stale_device_since"],
            serde_json::Value::Null,
            "auto-resume must leave no stale latch for the UI to prompt on",
        );
        // B still has the shared list (the snapshot re-pull preserved it).
        let lists: serde_json::Value =
            serde_json::from_str(&host_b.task_lists_json().unwrap()).unwrap();
        assert!(
            lists
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["id"] == serde_json::json!(list_id)),
            "B must still have the shared list after auto-resume; got {lists}",
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
            host.sync_now_json("manual".to_string()).unwrap();
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
        host2.sync_now_json("manual".to_string()).unwrap();
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
        host.sync_now_json("manual".to_string()).unwrap();

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
        host.sync_now_json("manual".to_string()).unwrap();

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
        host.sync_now_json("manual".to_string()).unwrap();

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
        host.sync_now_json("manual".to_string()).unwrap();

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
        host.sync_now_json("manual".to_string()).unwrap();
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
        host.sync_now_json("manual".to_string()).unwrap();

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
        host_a.sync_now_json("manual".to_string()).unwrap();

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
        host_b.sync_now_json("manual".to_string()).unwrap();
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
        host_a.sync_now_json("manual".to_string()).unwrap();

        // Rotate the passphrase; A keeps working (its keychain DEK is unchanged).
        host_a
            .change_sync_passphrase_json(OLD.to_string(), NEW.to_string())
            .unwrap();
        host_a.sync_now_json("manual".to_string()).unwrap();

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
        host_a.sync_now_json("manual".to_string()).unwrap();

        // B's next round hits the encryption gate: the target is encrypted but B
        // is still plaintext → EncryptionRequired, latched as the error code.
        assert!(
            host_b.sync_now_json("manual".to_string()).is_err(),
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
        host_b.sync_now_json("manual".to_string()).unwrap();
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

        // A local address book renames its own row (no override / sync event).
        let book: serde_json::Value = serde_json::from_str(
            &host
                .create_contact_list_json("Friends".to_string())
                .unwrap(),
        )
        .unwrap();
        let book_id = book["id"].as_str().unwrap().to_string();
        host.rename_container(
            book_id.clone(),
            "contact_list".to_string(),
            "Family".to_string(),
        )
        .unwrap();
        let books: serde_json::Value =
            serde_json::from_str(&host.contact_lists_json().unwrap()).unwrap();
        let renamed_book = books
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["id"] == serde_json::json!(book_id))
            .unwrap();
        assert_eq!(renamed_book["name"], "Family");

        // An unknown local book id → NotFound (nothing renamed).
        assert!(matches!(
            host.rename_container(
                "whatever".to_string(),
                "contact_list".to_string(),
                "X".to_string()
            )
            .unwrap_err(),
            StoreError::NotFound
        ));
    }

    #[test]
    fn contact_list_colour_uses_host_local_override() {
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

        // (Renaming a contact list now updates its own row, not an override —
        // covered by rename_container_renames_local_list_and_calendar.)

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
    fn search_finds_local_events_and_tasks() {
        let (_dir, host, _kc) = open_host();
        // A task whose title matches.
        let list: serde_json::Value =
            serde_json::from_str(&host.create_task_list_json("Work".to_string()).unwrap()).unwrap();
        let list_id = list["id"].as_str().unwrap().to_string();
        let new_task = r#"{"title":"Quarterly planning","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        host.create_task_json(list_id, new_task.to_string())
            .unwrap();
        // An event whose title matches.
        let cal: serde_json::Value = serde_json::from_str(
            &host
                .create_calendar_json(serde_json::json!({"name": "Personal"}).to_string())
                .unwrap(),
        )
        .unwrap();
        let cal_id = cal["id"].as_str().unwrap().to_string();
        host.create_event_json(new_event_json(&cal_id, "Planning offsite"))
            .unwrap();

        let results: serde_json::Value = serde_json::from_str(
            &host
                .search_json("planning".to_string(), String::new())
                .unwrap(),
        )
        .unwrap();
        assert!(results["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["title"] == serde_json::json!("Quarterly planning")));
        assert!(results["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["title"] == serde_json::json!("Planning offsite")));

        // A non-matching query returns nothing (+ empty query is short-circuited).
        let empty: serde_json::Value = serde_json::from_str(
            &host
                .search_json("zzznomatch".to_string(), String::new())
                .unwrap(),
        )
        .unwrap();
        assert!(empty["tasks"].as_array().unwrap().is_empty());
        assert!(empty["events"].as_array().unwrap().is_empty());
    }

    #[test]
    fn resolve_sync_conflict_take_remote_patches_the_row() {
        let (_dir, host, _kc) = open_host();
        // A local task titled "Local".
        let list: serde_json::Value =
            serde_json::from_str(&host.create_task_list_json("Work".to_string()).unwrap()).unwrap();
        let list_id = list["id"].as_str().unwrap().to_string();
        let new_task = r#"{"title":"Local","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let created: serde_json::Value = serde_json::from_str(
            &host
                .create_task_json(list_id.clone(), new_task.to_string())
                .unwrap(),
        )
        .unwrap();
        let task_id = created["id"].as_str().unwrap().to_string();

        // Record a field-level conflict on `title` (as the sync applier would).
        let conflict_id = {
            let shared = host.db.shared();
            let repo = ConflictsRepo::new(&shared);
            repo.record(host_core::conflicts::NewConflict {
                row_kind: ConflictKind::Task,
                row_id: task_id.clone(),
                field: "title".to_string(),
                local_value: Some(serde_json::to_string("Local").unwrap()),
                remote_value: Some(serde_json::to_string("Remote").unwrap()),
                remote_device_id: "device-2".to_string(),
                remote_timestamp: chrono::Utc::now(),
            })
            .unwrap()
        };

        // It lists + counts.
        assert_eq!(host.sync_conflict_count().unwrap(), 1);
        let listed: serde_json::Value =
            serde_json::from_str(&host.list_sync_conflicts_json().unwrap()).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["field"], serde_json::json!("title"));

        // take_remote writes the remote value into the row + resolves the conflict.
        host.resolve_sync_conflict(conflict_id, "take_remote".to_string())
            .unwrap();
        let tasks: serde_json::Value =
            serde_json::from_str(&host.tasks_json(list_id).unwrap()).unwrap();
        let patched = tasks
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == serde_json::json!(task_id))
            .unwrap();
        assert_eq!(
            patched["title"],
            serde_json::json!("Remote"),
            "title took the remote value"
        );
        assert_eq!(host.sync_conflict_count().unwrap(), 0, "conflict resolved");

        // save_both is not supported.
        let another = {
            let shared = host.db.shared();
            let repo = ConflictsRepo::new(&shared);
            repo.record(host_core::conflicts::NewConflict {
                row_kind: ConflictKind::Task,
                row_id: task_id,
                field: "description".to_string(),
                local_value: None,
                remote_value: Some(serde_json::to_string("note").unwrap()),
                remote_device_id: "device-2".to_string(),
                remote_timestamp: chrono::Utc::now(),
            })
            .unwrap()
        };
        assert!(matches!(
            host.resolve_sync_conflict(another, "save_both".to_string())
                .unwrap_err(),
            StoreError::Unsupported { .. }
        ));
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
        host_a.sync_now_json("manual".to_string()).unwrap();
        host_b.sync_now_json("manual".to_string()).unwrap();

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
    fn moving_a_task_between_local_lists_reroutes_it() {
        let (_dir, host, _kc) = open_host();
        let list_a = serde_json::from_str::<serde_json::Value>(
            &host.create_task_list_json("A".to_string()).unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let list_b = serde_json::from_str::<serde_json::Value>(
            &host.create_task_list_json("B".to_string()).unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let new_task = r#"{"title":"Movable","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let created = host
            .create_task_json(list_a.clone(), new_task.to_string())
            .unwrap();
        let mut task: serde_json::Value = serde_json::from_str(&created).unwrap();
        let task_id = task["id"].as_str().unwrap().to_string();

        // A local→local move passes the source as previous_list_id (the move
        // branch), which the local adapter resolves as a single SQL UPDATE on
        // the list_id column.
        task["list_id"] = serde_json::json!(list_b);
        host.update_task_json(task.to_string(), Some(list_a.clone()))
            .unwrap();

        // B now holds it; A no longer does.
        let in_b: serde_json::Value =
            serde_json::from_str(&host.tasks_json(list_b).unwrap()).unwrap();
        assert!(in_b
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == serde_json::json!(task_id)));
        let in_a: serde_json::Value =
            serde_json::from_str(&host.tasks_json(list_a).unwrap()).unwrap();
        assert!(in_a
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["id"] != serde_json::json!(task_id)));
    }

    #[test]
    fn updating_a_task_with_previous_list_equal_to_current_is_in_place() {
        let (_dir, host, _kc) = open_host();
        let list = serde_json::from_str::<serde_json::Value>(
            &host.create_task_list_json("L".to_string()).unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let new_task = r#"{"title":"Old","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let created = host
            .create_task_json(list.clone(), new_task.to_string())
            .unwrap();
        let mut task: serde_json::Value = serde_json::from_str(&created).unwrap();
        let task_id = task["id"].as_str().unwrap().to_string();
        // previous_list_id == list_id → in-place edit, not a move.
        task["title"] = serde_json::json!("Renamed");
        host.update_task_json(task.to_string(), Some(list.clone()))
            .unwrap();
        let reread: serde_json::Value =
            serde_json::from_str(&host.task_json(task_id).unwrap()).unwrap();
        assert_eq!(reread["title"], "Renamed");
    }

    #[test]
    fn editing_a_task_into_extras_recurrence_assigns_a_series_id() {
        // DESIGN §9.12: a plain task edited into an on-demand (backlog) recurring
        // one must gain a stable series_id host-side — the local adapter's
        // update path writes it verbatim and relies on the host having assigned
        // it. Without the hoist this in-place edit would persist series_id=null
        // and break the idempotent spawner's dedup.
        let (_dir, host, _kc) = open_host();
        let list = serde_json::from_str::<serde_json::Value>(
            &host.create_task_list_json("S".to_string()).unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let new_task = r#"{"title":"Plain","description":null,"status":"open","priority":"medium","scheduled_date":null,"scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let created = host
            .create_task_json(list.clone(), new_task.to_string())
            .unwrap();
        let mut task: serde_json::Value = serde_json::from_str(&created).unwrap();
        assert!(
            task["series_id"].is_null(),
            "a non-recurring task starts without a series id"
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        // Give it a backlog rule (placement != schedule ⇒ recurrence_needs_extras).
        task["recurrence"] = serde_json::json!({
            "frequency": "daily",
            "interval": 1,
            "day_of_week": null,
            "day_of_month": null,
            "end": null,
            "placement": "backlog"
        });
        // In-place edit (no move): previous == current.
        host.update_task_json(task.to_string(), Some(list)).unwrap();
        let reread: serde_json::Value =
            serde_json::from_str(&host.task_json(task_id).unwrap()).unwrap();
        assert!(
            reread["series_id"].is_string(),
            "editing into extras-recurrence assigns a series id; got {reread}"
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

    /// The merged Google consent asks for Drive too.
    ///
    /// It used to be the retired Drive plugin's own dance. Now one sign-in
    /// covers calendars, tasks, contacts AND the storage the account can hold,
    /// which is the whole point of the merge — and the `drive.file` assertion
    /// below is what proves the scope survived it.
    #[test]
    fn the_google_consent_asks_for_drive_as_well() {
        let (_dir, host, _kc) = open_host();
        let args = r#"{"client_id":"gd-client","redirect_uri":"aperio://oauth-callback"}"#;
        let out = host
            .begin_oauth_json(
                "com.aperio.cal-adapter-google".to_string(),
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
    fn birthday_calendar_synthesises_a_local_contacts_birthday() {
        let (_dir, host, _kc) = open_host();
        // Migration 0007 no longer seeds an address book; create one.
        let list_id = seed_contact_list(&host);
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
        // The fixed 2026 range makes the age deterministic: 2026 − 1990 = 36,
        // carried in the title (and thus in every reminder notification).
        assert_eq!(arr[0]["title"], serde_json::json!("Ada Lovelace (36)"));
        assert_eq!(arr[0]["all_day"], serde_json::json!(true));
    }

    #[test]
    fn contacts_round_trip_through_the_local_address_book() {
        let (_dir, host, _kc) = open_host();
        // Migration 0007 no longer seeds an address book; create one and
        // confirm it lists back on the local account.
        let seeded_id = seed_contact_list(&host);
        let lists: serde_json::Value =
            serde_json::from_str(&host.contact_lists_json().unwrap()).unwrap();
        let arr = lists.as_array().unwrap();
        assert!(!arr.is_empty(), "the created address book should list");
        let list_id = arr[0]["id"].as_str().unwrap().to_string();
        assert_eq!(list_id, seeded_id);
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
