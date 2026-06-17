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
//! SyncProgressBridge live-progress push callback + configure restore-on-open +
//! the E2E `wrap_if_encrypted` branch; task/list/section sync (those live on
//! the separate `LocalStore`, which folds into this Host later). External
//! event paths are wired like local but hit the provider live (no cache),
//! exercised on-device, not in unit tests.

use std::sync::Arc;

use cal_adapter_local::LocalAdapter;
use cal_core::{Calendar, CalendarFeature, ColorLabelId, DateRange, Event, NewEvent};
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
/// Plugin id of the local-filesystem sync adapter (the only kind this slice
/// configures; webdav/sftp/ftp + the OAuth kinds follow).
const PLUGIN_ID_SYNC_LOCAL: &str = "com.aperio.sync-adapter-local";

fn sync_err(e: SyncError) -> StoreError {
    StoreError::Storage {
        detail: e.to_string(),
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

/// Configure-sync request. A subset of the desktop `SyncAdapterConfig` enum —
/// this slice handles `kind: "local"` (path required); webdav/sftp/ftp + the
/// OAuth kinds follow.
#[derive(serde::Deserialize)]
struct ConfigureSyncRequest {
    kind: String,
    #[serde(default)]
    path: Option<String>,
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

    /// Open a statically-embedded sync-adapter plugin instance + wrap it as a
    /// `SyncAdapter` (the desktop `open_sync_plugin` pattern). Used by
    /// `configure_sync_adapter_json`.
    fn open_sync_plugin(
        &self,
        plugin_id: &str,
        config_json: String,
    ) -> Result<Arc<dyn SyncAdapter>, StoreError> {
        let plugin = self
            .plugin_manager
            .get(plugin_id)
            .ok_or_else(|| StoreError::Storage {
                detail: format!("sync plugin {plugin_id} is not loaded"),
            })?;
        let instance = self
            .plugin_manager
            .open_instance(plugin, &config_json)
            .map_err(|e| StoreError::Storage {
                detail: format!("open sync plugin {plugin_id}: {e}"),
            })?;
        let adapter = FfiSyncAdapter::new(instance).ok_or_else(|| StoreError::Storage {
            detail: format!("plugin {plugin_id} has no SyncAdapter surface"),
        })?;
        Ok(Arc::new(adapter))
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

    /// The orchestrator's status as JSON (the desktop `SyncStatus` shape:
    /// configured / in_flight / last_synced_at / interval / e2e / …). Reads
    /// without a sync round.
    pub fn sync_status_json(&self) -> Result<String, StoreError> {
        to_json(&self.orchestrator.status())
    }

    /// Configure the sync adapter from a JSON request. This slice handles
    /// `{"kind":"local","path":"…"}` (a local-filesystem sync target): open the
    /// statically-embedded local sync plugin, probe it (`test_connection`), make
    /// it the orchestrator's active adapter, and persist the choice under the
    /// `sync.adapter.*` prefs (device-local; the is_synced_key allowlist
    /// excludes them, so they never propagate). webdav/sftp/ftp + the E2E
    /// `wrap_if_encrypted` branch + the OAuth kinds follow.
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
                let adapter = self.open_sync_plugin(PLUGIN_ID_SYNC_LOCAL, cfg)?;
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
            other => Err(StoreError::InvalidField {
                field: "kind".to_string(),
                detail: format!("sync adapter kind '{other}' is not supported yet (local only)"),
            }),
        }
    }

    /// Run one sync round (push local pending logs, fetch + apply foreign ones,
    /// compaction audit) and return the `SyncRoundReport` as JSON. Errors with
    /// "not configured" until `configure_sync_adapter_json` has run.
    pub fn sync_now_json(&self) -> Result<String, StoreError> {
        let report = self
            .runtime
            .block_on(async { self.orchestrator.sync_now().await })
            .map_err(sync_err)?;
        to_json(&report)
    }

    /// Push the local pending logs without fetching (call from RN AppState
    /// "background"). Returns the number of logs pushed.
    pub fn push_now(&self) -> Result<u32, StoreError> {
        let pushed = self
            .runtime
            .block_on(async { self.orchestrator.push_now().await })
            .map_err(sync_err)?;
        Ok(pushed as u32)
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
        AccountsError::UnknownKind(detail) => StoreError::InvalidField {
            field: "adapter_kind".to_string(),
            detail,
        },
        AccountsError::Sqlite(e) => StoreError::Storage {
            detail: e.to_string(),
        },
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
}
