//! Adapter registry — routing layer between Tauri commands and the
//! concrete adapter crates.
//!
//! Phase 6a parked every adapter assumption inside the local
//! adapter. With CalDAV (and later Google / MS Graph) coming
//! online we need a layer that:
//!
//!   1. Constructs one [`Adapter`] instance per persisted account
//!      and keeps it alive for the lifetime of the app.
//!   2. Holds a reverse map `calendar_id → account_id` (and the
//!      same for task lists) so a single string identifier on the
//!      wire is enough to route a write back to the originating
//!      account.
//!   3. Aggregates the read side of the CalendarFeature /
//!      TasksFeature traits across all accounts so the existing
//!      command surface (`list_calendars`, `list_task_lists`)
//!      stays single-shot from the frontend's point of view.
//!
//! The local adapter is *not* registered here. It already lives in
//! Tauri state as a typed `LocalAdapter` and the commands call it
//! directly for the `local` account. External adapters all sit
//! behind `Arc<dyn CalendarFeature>` / `Arc<dyn TasksFeature>` so
//! the registry can grow new adapter kinds without changing the
//! routing code.
//!
//! ## Plugin routing (ABI v2)
//!
//! Every `register_*` fn loads the matching plugin via
//! [`plugin_core::PluginManager`] and opens a fresh per-account
//! instance with the account's JSON config + the secrets pulled
//! from the platform keychain. The host wraps that instance in
//! [`plugin_core::shim::FfiCalendarAdapter`] / `FfiTasksAdapter` /
//! `FfiContactsAdapter` so the rest of the host sees the same
//! `Arc<dyn CalendarFeature>` trait surface as before.
//!
//! Single-binary plugin registration uses
//! [`plugin_core::PluginManager::register_static`] so we avoid
//! the dlopen pipeline for now — DESIGN.md §22.2's `plugins/
//! bundled/` build step lands in a later phase.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use plugin_core::shim::{FfiCalendarAdapter, FfiContactsAdapter, FfiTasksAdapter, FfiVcAdapter};
use plugin_core::{LoadedInstance, LoadedPlugin, PluginManager};
use serde_json::{json, Map, Value};
use sync_engine::{SecretSlot, SecretStore};
use tracing::warn;
use vc_core::VcAdapter;

use crate::accounts::{Account, AccountsRepo, AdapterKind, LOCAL_ACCOUNT_ID};

/// Account-id used for the implicit local adapter. Mirrors the value
/// the `accounts` table seeds during migration 0003.
pub const LOCAL_ID: &str = LOCAL_ACCOUNT_ID;

/// Plugin-id constants for the adapters still on the per-kind registration
/// path below.
///
/// NOT the kind→plugin map any more — that comes from the manifests, via
/// [`plugin_core::PluginManager::plugin_for_adapter_kind`]. These are only what
/// the remaining hand-written `register_*` functions open. Each one disappears
/// with its function as its adapter declares an account schema; Webex's already
/// has.
const PLUGIN_ID_CALDAV: &str = "com.aperio.cal-adapter-caldav";
const PLUGIN_ID_ICAL: &str = "com.aperio.cal-adapter-ical";
const PLUGIN_ID_GOOGLE: &str = "com.aperio.cal-adapter-google";
const PLUGIN_ID_GRAPH: &str = "com.aperio.cal-adapter-microsoft-graph";
const PLUGIN_ID_EWS: &str = "com.aperio.cal-adapter-ews";
const PLUGIN_ID_VIKUNJA: &str = "com.aperio.cal-adapter-vikunja";
const PLUGIN_ID_TODOIST: &str = "com.aperio.cal-adapter-todoist";
const PLUGIN_ID_ZOOM: &str = "com.aperio.vc-adapter-zoom";
const PLUGIN_ID_TEAMS: &str = "com.aperio.vc-adapter-teams";
const PLUGIN_ID_MEET: &str = "com.aperio.vc-adapter-meet";

/// Tracks which account a calendar / task-list came from so writes
/// can find their way home. Filled lazily during the first
/// `list_calendars` / `list_task_lists` call after startup and
/// refreshed on every subsequent one.
#[derive(Debug, Default)]
struct Routes {
    calendar_to_account: HashMap<String, String>,
    list_to_account: HashMap<String, String>,
    /// Contact-list id → account id. Same shape as the others;
    /// filled lazily during the first `list_contact_lists` call
    /// and refreshed on every subsequent one.
    contact_list_to_account: HashMap<String, String>,
}

/// Process-wide registry of all non-local adapter instances.
pub struct AdapterRegistry {
    /// External adapters with CalendarFeature, keyed by account_id.
    external_cal: RwLock<HashMap<String, Arc<dyn CalendarFeature>>>,
    /// External adapters with TasksFeature, keyed by account_id.
    external_tasks: RwLock<HashMap<String, Arc<dyn TasksFeature>>>,
    /// External adapters with ContactsFeature, keyed by account_id.
    external_contacts: RwLock<HashMap<String, Arc<dyn ContactsFeature>>>,
    /// Videoconference adapters, keyed by account_id. Separate
    /// map (rather than a slot on the cal/tasks/contacts shape)
    /// because vc-adapters don't share their account row with
    /// the calendar adapters they accompany — Teams + Meet
    /// share OAuth tokens with their cal-adapter siblings but
    /// each lives on its own `accounts` row with its own
    /// adapter_kind.
    external_vc: RwLock<HashMap<String, Arc<dyn VcAdapter>>>,
    /// Reverse lookup for routing writes back to the right adapter.
    routes: Mutex<Routes>,
    /// Accounts whose registration has already failed and been warned
    /// about. The post-sync sweep retries them every round (credentials
    /// can arrive at any time), so this keeps that retry from spamming a
    /// WARN per round; cleared for an account the moment it registers.
    unregisterable: Mutex<HashSet<String>>,
    /// Loaded plugins keyed by their canonical id. Every
    /// `register_*` fn pulls the matching plugin out of here +
    /// opens a fresh per-account instance against it. Empty on
    /// the test path (in which case `register_*` calls fail
    /// cleanly with `RegistryError::PluginMissing`).
    plugin_manager: Arc<PluginManager>,
    /// Root for plugin-side persistent state — per-account
    /// `<data_dir>/plugin_state/<plugin>/<account_id>/` directories
    /// are computed off this and spliced into the InitConfig at
    /// open-instance time. Plugins that opt into a `state_dir`
    /// field (EWS, today) get a stable per-account location for
    /// caches, sync cookies, etc. Other plugins ignore the field.
    /// `None` on the test path so legacy `AdapterRegistry::new`
    /// call sites keep working without plumbing a temp dir.
    data_dir: Option<std::path::PathBuf>,
    /// Platform secret store (the injected seam). Desktop passes a
    /// keyring-backed impl; mobile passes a Keychain/Keystore bridge.
    /// The `register_*` builders read each account's credentials
    /// through this instead of a hard-coded keyring call, so the
    /// registry stays Tauri-/platform-free.
    secret_store: Arc<dyn SecretStore>,
    /// Host-channel capability tokens, keyed by account id.
    ///
    /// A token retires the moment it is dropped, so it has to outlive the
    /// instance it was minted for; parking it here ties its life to the
    /// registration. Re-registering an account replaces its token, which
    /// retires the old one into the grace ring where a report that was already
    /// in flight can still land.
    scope_tokens: Mutex<HashMap<String, plugin_core::host_channel::ScopeToken>>,
}

impl AdapterRegistry {
    /// Construct a registry against the host's plugin manager.
    /// Production callers pass the `Arc<PluginManager>` built at
    /// app startup (after `register_static` ran for every
    /// bundled plugin); tests can pass an empty
    /// `PluginManager::new("0.1.0")` when they don't exercise the
    /// register / bootstrap path.
    pub fn new(plugin_manager: Arc<PluginManager>, secret_store: Arc<dyn SecretStore>) -> Self {
        Self::with_data_dir(plugin_manager, secret_store, None)
    }

    /// Variant that records the host's data directory so per-
    /// account plugin state (sync cookies + cached items) can be
    /// persisted across restarts. Production startup uses this;
    /// tests can still call `new(plugin_manager)` and accept the
    /// "no persistence" fallback.
    pub fn with_data_dir(
        plugin_manager: Arc<PluginManager>,
        secret_store: Arc<dyn SecretStore>,
        data_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            external_cal: RwLock::new(HashMap::new()),
            external_tasks: RwLock::new(HashMap::new()),
            external_contacts: RwLock::new(HashMap::new()),
            external_vc: RwLock::new(HashMap::new()),
            routes: Mutex::new(Routes::default()),
            unregisterable: Mutex::new(HashSet::new()),
            plugin_manager,
            data_dir,
            secret_store,
            scope_tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Compute the per-account state directory for `plugin_id`,
    /// creating it on disk if it doesn't exist yet. Returns
    /// `None` when the registry was built without a data_dir
    /// (test path) or when directory creation fails — the
    /// adapter then falls back to in-memory state, exactly as
    /// before the persistence work landed.
    fn plugin_state_dir(&self, plugin_id: &str, account_id: &str) -> Option<std::path::PathBuf> {
        let root = self.data_dir.as_ref()?;
        let dir = root.join("plugin_state").join(plugin_id).join(account_id);
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(
                ?err,
                path = %dir.display(),
                "couldn't create plugin state dir; persistence disabled",
            );
            return None;
        }
        Some(dir)
    }

    /// Build adapters for every persisted external account. Failures
    /// per account are logged and skipped so one broken row doesn't
    /// stop the rest of the app from booting.
    pub fn bootstrap(&self, repo: &AccountsRepo<'_>) {
        self.register_persisted(repo, false);
    }

    /// Register adapters for persisted accounts that currently have NO
    /// adapter, and return how many were newly registered.
    ///
    /// Accounts that arrive through SYNC (a restore into a fresh install,
    /// or an account added on another device) are written straight into
    /// the `accounts` table by the event-log applier — they never pass
    /// through the add-account commands that call [`Self::register`], so
    /// their adapter only ever comes up at the next [`Self::bootstrap`].
    /// Until then the sidebar shows the account name with no containers
    /// and no items. Hosts call this after a sync round that applied
    /// something to close that gap without an app restart.
    ///
    /// Deliberately NOT a blanket re-bootstrap: re-registering a live
    /// account rebuilds its plugin instance and throws away the adapter's
    /// in-memory provider state, so e.g. EWS would cold-start and re-drain
    /// every item. Only adapter-less accounts are touched. Accounts whose
    /// credentials don't exist on this device fail `try_register` — that's
    /// expected (the reconnect wizard covers them); they're logged and
    /// skipped, exactly as at bootstrap.
    pub fn register_missing(&self, repo: &AccountsRepo<'_>) -> usize {
        self.register_persisted(repo, true)
    }

    /// Shared body of [`Self::bootstrap`] + [`Self::register_missing`].
    /// `only_missing` skips accounts that already have an adapter.
    /// Returns the number of accounts newly registered.
    fn register_persisted(&self, repo: &AccountsRepo<'_>, only_missing: bool) -> usize {
        let phase = if only_missing {
            "post-sync"
        } else {
            "bootstrap"
        };
        let accounts = match repo.list() {
            Ok(a) => a,
            Err(err) => {
                warn!(?err, phase, "failed to list accounts");
                return 0;
            }
        };
        let mut registered = 0usize;
        for account in accounts {
            // Local is host-internal; DeviceCalendar is built + inserted by the
            // cal-ffi layer once its native bridge is set. Neither registers here.
            if account.adapter_kind.is_host_internal() {
                continue;
            }
            if only_missing && self.has_adapter(&account.id) {
                continue;
            }
            match self.try_register(&account) {
                Ok(()) => {
                    registered += 1;
                    self.unregisterable
                        .lock()
                        .expect("registry poisoned")
                        .remove(&account.id);
                    // INFO-level so a user diagnosing "calendar X
                    // shows no events" can verify the adapter
                    // actually came up for that account.
                    tracing::info!(
                        target: "aperio::registry",
                        account_id = %account.id,
                        kind = ?account.adapter_kind,
                        display_name = %account.display_name,
                        phase,
                        "registered external account adapter",
                    );
                }
                Err(err) => {
                    // An account that CANNOT register (credentials absent on
                    // this device — the §19.11 reconnect wizard's job) fails
                    // again on every single pass. `bootstrap` runs once, so
                    // it warns; the post-sync sweep runs after every round,
                    // so warning each time would be pure log spam. Warn once
                    // per account, then drop to debug until it succeeds (a
                    // successful registration removes it from the set, so a
                    // later regression warns again).
                    let first = self
                        .unregisterable
                        .lock()
                        .expect("registry poisoned")
                        .insert(account.id.clone());
                    if first {
                        warn!(
                            account_id = %account.id,
                            kind = ?account.adapter_kind,
                            ?err,
                            phase,
                            "skipping account"
                        );
                    } else {
                        tracing::debug!(
                            account_id = %account.id,
                            kind = ?account.adapter_kind,
                            ?err,
                            phase,
                            "still skipping account"
                        );
                    }
                }
            }
        }
        registered
    }

    /// Whether `account_id` already has an adapter on ANY feature surface.
    /// All four maps are consulted because the surfaces an account fills
    /// depend on its kind (VC accounts only ever land in `external_vc`,
    /// Todoist only in `external_tasks`, …).
    fn has_adapter(&self, account_id: &str) -> bool {
        self.external_cal
            .read()
            .expect("registry cal poison")
            .contains_key(account_id)
            || self
                .external_tasks
                .read()
                .expect("registry tasks poison")
                .contains_key(account_id)
            || self
                .external_contacts
                .read()
                .expect("registry contacts poison")
                .contains_key(account_id)
            || self
                .external_vc
                .read()
                .expect("registry vc poison")
                .contains_key(account_id)
    }

    /// Register a single account at runtime. Used by
    /// `create_account` after a successful authentication smoke
    /// test so the new adapter becomes routable immediately
    /// without an app restart.
    pub fn register(&self, account: &Account) -> Result<(), RegistryError> {
        self.try_register(account)
    }

    /// Insert a host-constructed (non-plugin) adapter's feature surfaces
    /// directly under `account_id`. The mobile device-calendar adapter is built
    /// in the cal-ffi layer — it wraps the injected native EventStore bridge, so
    /// it can't come through the plugin manager — and registers itself here.
    /// `cal` and `tasks` are usually clones of the same `Arc<DeviceAdapter>`
    /// coerced to each trait object (it implements both). Re-registering an
    /// account id overwrites the prior entry. Routes are filled lazily by the
    /// next `list_calendars` / `list_task_lists`, as for plugin adapters.
    pub fn register_host_adapter(
        &self,
        account_id: &str,
        cal: Option<Arc<dyn CalendarFeature>>,
        tasks: Option<Arc<dyn TasksFeature>>,
    ) {
        if let Some(cal) = cal {
            self.external_cal
                .write()
                .expect("registry cal poison")
                .insert(account_id.to_string(), cal);
        }
        if let Some(tasks) = tasks {
            self.external_tasks
                .write()
                .expect("registry tasks poison")
                .insert(account_id.to_string(), tasks);
        }
    }

    /// Drop the adapter for `account_id`. Called from
    /// `delete_account` so the registry doesn't hand out a stale
    /// reference after deletion.
    pub fn unregister(&self, account_id: &str) {
        self.external_cal
            .write()
            .expect("registry cal poison")
            .remove(account_id);
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .remove(account_id);
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .remove(account_id);
        self.external_vc
            .write()
            .expect("registry vc poison")
            .remove(account_id);
        let mut routes = self.routes.lock().expect("registry routes poison");
        routes
            .calendar_to_account
            .retain(|_, owner| owner != account_id);
        routes
            .list_to_account
            .retain(|_, owner| owner != account_id);
        routes
            .contact_list_to_account
            .retain(|_, owner| owner != account_id);
    }

    /// Look up the account-id that owns `calendar_id`. Returns
    /// `None` when the id has not been seen by `list_calendars`
    /// since startup — in that case the calling command can fall
    /// back to "assume local".
    pub fn account_for_calendar(&self, calendar_id: &str) -> Option<String> {
        self.routes
            .lock()
            .expect("registry routes poison")
            .calendar_to_account
            .get(calendar_id)
            .cloned()
    }

    pub fn account_for_task_list(&self, list_id: &str) -> Option<String> {
        self.routes
            .lock()
            .expect("registry routes poison")
            .list_to_account
            .get(list_id)
            .cloned()
    }

    pub fn account_for_contact_list(&self, list_id: &str) -> Option<String> {
        self.routes
            .lock()
            .expect("registry routes poison")
            .contact_list_to_account
            .get(list_id)
            .cloned()
    }

    /// Snapshot every registered CalendarFeature adapter together with
    /// its owning account id. The lock is dropped before the caller
    /// awaits anything on the adapters, so a slow adapter doesn't
    /// block concurrent `register` / `unregister` calls from
    /// `create_account` / `delete_account`.
    pub fn snapshot_calendar_adapters(&self) -> Vec<(String, Arc<dyn CalendarFeature>)> {
        self.external_cal
            .read()
            .expect("registry cal poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn snapshot_task_adapters(&self) -> Vec<(String, Arc<dyn TasksFeature>)> {
        self.external_tasks
            .read()
            .expect("registry tasks poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn snapshot_contact_adapters(&self) -> Vec<(String, Arc<dyn ContactsFeature>)> {
        self.external_contacts
            .read()
            .expect("registry contacts poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Run `list_calendars` on every external CalendarFeature
    /// adapter and return the flat aggregated list. Errors from
    /// one adapter don't poison the rest; they are logged and the
    /// other accounts still get to show up.
    pub async fn list_external_calendars(&self) -> Vec<cal_core::Calendar> {
        let snapshot = self.snapshot_calendar_adapters();
        tracing::info!(
            target: "aperio::registry",
            adapter_count = snapshot.len(),
            "list_external_calendars iterating registered CalendarFeature adapters",
        );
        let mut out = Vec::new();
        for (account_id, adapter) in snapshot {
            match adapter.list_calendars().await {
                Ok(cals) => {
                    tracing::info!(
                        target: "aperio::registry",
                        account_id = %account_id,
                        count = cals.len(),
                        "list_calendars returned",
                    );
                    let mut routes = self.routes.lock().expect("registry routes poison");
                    for c in &cals {
                        routes
                            .calendar_to_account
                            .insert(c.id.clone(), account_id.clone());
                    }
                    out.extend(cals);
                }
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "list_calendars failed for external adapter"
                    );
                }
            }
        }
        out
    }

    pub async fn list_external_task_lists(&self) -> Vec<cal_core::TaskList> {
        let snapshot = self.snapshot_task_adapters();
        let mut out = Vec::new();
        for (account_id, adapter) in snapshot {
            match adapter.list_task_lists().await {
                Ok(lists) => {
                    let mut routes = self.routes.lock().expect("registry routes poison");
                    for l in &lists {
                        routes
                            .list_to_account
                            .insert(l.id.clone(), account_id.clone());
                    }
                    out.extend(lists);
                }
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "list_task_lists failed for external adapter"
                    );
                }
            }
        }
        out
    }

    pub async fn list_external_contact_lists(&self) -> Vec<cal_core::ContactList> {
        let snapshot = self.snapshot_contact_adapters();
        let mut out = Vec::new();
        for (account_id, adapter) in snapshot {
            match adapter.list_contact_lists().await {
                Ok(lists) => {
                    let mut routes = self.routes.lock().expect("registry routes poison");
                    for l in &lists {
                        routes
                            .contact_list_to_account
                            .insert(l.id.clone(), account_id.clone());
                    }
                    out.extend(lists);
                }
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "list_contact_lists failed for external adapter"
                    );
                }
            }
        }
        out
    }

    pub async fn search_external_contacts(&self, query: &str) -> Vec<cal_core::Contact> {
        let snapshot = self.snapshot_contact_adapters();
        let mut out = Vec::new();
        for (account_id, adapter) in snapshot {
            match adapter.search_contacts(query).await {
                Ok(hits) => out.extend(hits),
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "search_contacts failed for external adapter"
                    );
                }
            }
        }
        out
    }

    pub fn calendar_adapter(&self, account_id: &str) -> Option<Arc<dyn CalendarFeature>> {
        self.external_cal
            .read()
            .expect("registry cal poison")
            .get(account_id)
            .cloned()
    }

    pub fn task_adapter(&self, account_id: &str) -> Option<Arc<dyn TasksFeature>> {
        self.external_tasks
            .read()
            .expect("registry tasks poison")
            .get(account_id)
            .cloned()
    }

    pub fn contact_adapter(&self, account_id: &str) -> Option<Arc<dyn ContactsFeature>> {
        self.external_contacts
            .read()
            .expect("registry contacts poison")
            .get(account_id)
            .cloned()
    }

    /// Borrow the registered videoconference adapter for
    /// `account_id`, or `None` when nothing is registered.
    /// Used by the `create_meeting` / `delete_meeting` Tauri
    /// commands.
    pub fn vc_adapter(&self, account_id: &str) -> Option<Arc<dyn VcAdapter>> {
        self.external_vc
            .read()
            .expect("registry vc poison")
            .get(account_id)
            .cloned()
    }

    /// Snapshot every registered VcAdapter. Drives the
    /// AccountsDialog's "which providers do I have signed in?"
    /// rendering when the user picks "Generate meeting link"
    /// on a new event.
    pub fn snapshot_vc_adapters(&self) -> Vec<(String, Arc<dyn VcAdapter>)> {
        self.external_vc
            .read()
            .expect("registry vc poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Record that a calendar id has been observed against
    /// `account_id`. Used by `list_calendars` to register the
    /// local adapter's rows so subsequent get_events calls can
    /// route them home the same way external rows are routed.
    pub fn note_calendar_route(&self, calendar_id: &str, account_id: &str) {
        self.routes
            .lock()
            .expect("registry routes poison")
            .calendar_to_account
            .insert(calendar_id.to_string(), account_id.to_string());
    }

    pub fn note_task_list_route(&self, list_id: &str, account_id: &str) {
        self.routes
            .lock()
            .expect("registry routes poison")
            .list_to_account
            .insert(list_id.to_string(), account_id.to_string());
    }

    pub fn note_contact_list_route(&self, list_id: &str, account_id: &str) {
        self.routes
            .lock()
            .expect("registry routes poison")
            .contact_list_to_account
            .insert(list_id.to_string(), account_id.to_string());
    }

    // ─────────────────────────────────────────────────────────────
    // Plugin-routing helpers
    // ─────────────────────────────────────────────────────────────

    /// Resolve a plugin by id or surface a clear `PluginMissing`
    /// error. The §20.8 "Plugin fehlt" UX hook is what surfaces
    /// these to the user.
    fn require_plugin(&self, plugin_id: &str) -> Result<Arc<LoadedPlugin>, RegistryError> {
        self.plugin_manager
            .get(plugin_id)
            .ok_or_else(|| RegistryError::PluginMissing(plugin_id.to_string()))
    }

    /// Open an instance of the named plugin with the supplied JSON
    /// config. Maps plugin-side errors into the registry error
    /// type so callers can handle them uniformly.
    /// Park a capability token for the lifetime of this account's
    /// registration. Replacing one retires its predecessor.
    fn retain_scope(&self, account_id: &str, scope: plugin_core::host_channel::ScopeToken) {
        self.scope_tokens
            .lock()
            .expect("scope token map poisoned")
            .insert(account_id.to_string(), scope);
    }

    fn open_plugin_instance(
        &self,
        plugin_id: &str,
        config_json: String,
    ) -> Result<Arc<LoadedInstance>, RegistryError> {
        let plugin = self.require_plugin(plugin_id)?;
        self.plugin_manager
            .open_instance(plugin, &config_json)
            .map_err(|err| RegistryError::Construct(err.to_string()))
    }

    /// Smoke-test entered credentials WITHOUT persisting: open an EPHEMERAL
    /// instance from (kind + config + secret) — the same per-kind config the
    /// `register_*` paths build — run the kind's read probe (list_calendars /
    /// list_task_lists), then drop it. `Ok(())` = the credentials work.
    /// `Unsupported` for kinds with no credential probe (Local / OAuth); the
    /// caller treats that as "nothing to smoke". Mirrors the desktop
    /// `test_*_connection` commands.
    pub async fn probe_account(
        &self,
        adapter_kind: &AdapterKind,
        config_json: &str,
        secret: Option<&str>,
    ) -> Result<(), RegistryError> {
        enum ProbeFeature {
            Calendar,
            Tasks,
        }
        let (plugin_id, feature, config) = match adapter_kind.as_str() {
            "caldav" => {
                let secret =
                    secret.ok_or_else(|| RegistryError::Secret("missing password".into()))?;
                (
                    PLUGIN_ID_CALDAV,
                    ProbeFeature::Calendar,
                    merge_account_config(
                        config_json,
                        &[("secret", Value::String(secret.to_string()))],
                    )?,
                )
            }
            "ical" => {
                // An iCal feed may be public — an empty password stays null,
                // mirroring register_ical.
                let password = secret
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::String(s.to_string()))
                    .unwrap_or(Value::Null);
                (
                    PLUGIN_ID_ICAL,
                    ProbeFeature::Calendar,
                    merge_account_config(config_json, &[("password", password)])?,
                )
            }
            "ews" => {
                let secret =
                    secret.ok_or_else(|| RegistryError::Secret("missing password".into()))?;
                // No state_dir here (unlike register_ews) — a probe never
                // persists; EwsAccountConfig fills it from a serde default.
                (
                    PLUGIN_ID_EWS,
                    ProbeFeature::Calendar,
                    merge_account_config(
                        config_json,
                        &[("password", Value::String(secret.to_string()))],
                    )?,
                )
            }
            "vikunja" => {
                let secret =
                    secret.ok_or_else(|| RegistryError::Secret("missing API token".into()))?;
                (
                    PLUGIN_ID_VIKUNJA,
                    ProbeFeature::Tasks,
                    merge_account_config(
                        config_json,
                        &[("token", Value::String(secret.to_string()))],
                    )?,
                )
            }
            "todoist" => {
                let secret =
                    secret.ok_or_else(|| RegistryError::Secret("missing API token".into()))?;
                (
                    PLUGIN_ID_TODOIST,
                    ProbeFeature::Tasks,
                    merge_account_config(
                        config_json,
                        &[("token", Value::String(secret.to_string()))],
                    )?,
                )
            }
            other => return Err(RegistryError::Unsupported(other.to_string())),
        };
        let instance = self.open_plugin_instance(plugin_id, config)?;
        match feature {
            ProbeFeature::Calendar => {
                let adapter = FfiCalendarAdapter::new(instance).ok_or_else(|| {
                    RegistryError::Construct(
                        "plugin doesn't expose the CalendarFeature surface".into(),
                    )
                })?;
                adapter
                    .list_calendars()
                    .await
                    .map(|_| ())
                    .map_err(|err| RegistryError::Probe(err.to_string()))
            }
            ProbeFeature::Tasks => {
                let adapter = FfiTasksAdapter::new(instance).ok_or_else(|| {
                    RegistryError::Construct(
                        "plugin doesn't expose the TasksFeature surface".into(),
                    )
                })?;
                adapter
                    .list_task_lists()
                    .await
                    .map(|_| ())
                    .map_err(|err| RegistryError::Probe(err.to_string()))
            }
        }
    }

    /// Insert the calendar slot for `account_id`. Wraps the
    /// FfiAdapter::new failure mode into a clear error so
    /// downstream callers (the AccountsDialog "this account
    /// can't be loaded" hint) see why.
    fn insert_calendar(
        &self,
        account_id: &str,
        instance: Arc<LoadedInstance>,
    ) -> Result<(), RegistryError> {
        let adapter = FfiCalendarAdapter::new(instance).ok_or_else(|| {
            RegistryError::Construct("plugin doesn't expose the CalendarFeature surface".into())
        })?;
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(account_id.to_string(), Arc::new(adapter));
        Ok(())
    }

    fn insert_tasks(
        &self,
        account_id: &str,
        instance: Arc<LoadedInstance>,
    ) -> Result<(), RegistryError> {
        let adapter = FfiTasksAdapter::new(instance).ok_or_else(|| {
            RegistryError::Construct("plugin doesn't expose the TasksFeature surface".into())
        })?;
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(account_id.to_string(), Arc::new(adapter));
        Ok(())
    }

    fn insert_contacts(
        &self,
        account_id: &str,
        instance: Arc<LoadedInstance>,
    ) -> Result<(), RegistryError> {
        let adapter = FfiContactsAdapter::new(instance).ok_or_else(|| {
            RegistryError::Construct("plugin doesn't expose the ContactsFeature surface".into())
        })?;
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .insert(account_id.to_string(), Arc::new(adapter));
        Ok(())
    }

    fn insert_vc(
        &self,
        account_id: &str,
        instance: Arc<LoadedInstance>,
    ) -> Result<(), RegistryError> {
        let adapter = FfiVcAdapter::new(instance).ok_or_else(|| {
            RegistryError::Construct("plugin doesn't expose the VcAdapter surface".into())
        })?;
        self.external_vc
            .write()
            .expect("registry vc poison")
            .insert(account_id.to_string(), Arc::new(adapter));
        Ok(())
    }

    fn try_register(&self, account: &Account) -> Result<(), RegistryError> {
        // Host-internal kinds have no plugin and never did: the local store is
        // built in, and the device calendar is built in the cal-ffi layer over
        // a native bridge. Both are inserted by their own host code.
        if account.adapter_kind.is_host_internal() {
            return Ok(());
        }

        // Which plugin serves this kind is the PLUGIN's statement, read from
        // the loaded manifests. The host carries no table — that table was what
        // forced an edit to the core before any adapter could exist.
        let plugin = self
            .plugin_manager
            .plugin_for_adapter_kind(account.adapter_kind.as_str());

        // A plugin that also declares an account schema registers generically:
        // the schema says which secrets it wants and what to call them, so an
        // adapter Aperio has never seen opens exactly like one it ships.
        if let Some(plugin) = &plugin {
            if let Some(schema) = plugin.manifest.account.clone() {
                return self.register_from_schema(account, &plugin.manifest.id.clone(), &schema);
            }
        }

        // The older per-kind path, for the adapters that have not declared a
        // schema yet. Each can leave this list by adding the block to its own
        // `plugin.json`; nothing here has to change when it does.
        match account.adapter_kind.as_str() {
            "caldav" => self.register_caldav(account),
            "ical" => self.register_ical(account),
            "google" => self.register_google(account),
            "microsoft_graph" => self.register_microsoft_graph(account),
            "ews" => self.register_ews(account),
            "vikunja" => self.register_vikunja(account),
            "todoist" => self.register_todoist(account),
            "zoom" => self.register_zoom(account),
            "teams" => self.register_teams(account),
            "meet" => self.register_meet(account),
            // Not host-internal, no plugin loaded for it, and not one of the
            // kinds this build still knows by name. That is a plugin that is
            // missing or switched off — including an account synced from a
            // device with an adapter this build does not have — and the
            // Accounts panel already renders exactly that. The row is kept.
            other => Err(RegistryError::PluginMissing(format!(
                "no plugin serves adapter kind `{other}`"
            ))),
        }
    }

    fn register_caldav(&self, account: &Account) -> Result<(), RegistryError> {
        let secret = self
            .secret_store
            .retrieve(&account.id, SecretSlot::Password)
            .map_err(|e| RegistryError::Secret(format!("missing password: {e}")))?;
        let plugin_config =
            merge_account_config(&account.config_json, &[("secret", Value::String(secret))])?;
        let instance = self.open_plugin_instance(PLUGIN_ID_CALDAV, plugin_config)?;
        // The same LoadedInstance Arc is cloned into all three
        // FfiAdapters — the underlying CaldavAdapter inside the
        // plugin is shared so discovery + listing caches stay
        // coherent across the three feature surfaces.
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_google(&self, account: &Account) -> Result<(), RegistryError> {
        let plugin_config = oauth_plugin_config(
            self.secret_store.as_ref(),
            &account.id,
            &account.config_json,
        )?;
        let instance = self.open_plugin_instance(PLUGIN_ID_GOOGLE, plugin_config)?;
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_microsoft_graph(&self, account: &Account) -> Result<(), RegistryError> {
        let plugin_config = oauth_plugin_config(
            self.secret_store.as_ref(),
            &account.id,
            &account.config_json,
        )?;
        let instance = self.open_plugin_instance(PLUGIN_ID_GRAPH, plugin_config)?;
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_ews(&self, account: &Account) -> Result<(), RegistryError> {
        let password = self
            .secret_store
            .retrieve(&account.id, SecretSlot::Password)
            .map_err(|e| RegistryError::Secret(format!("missing password: {e}")))?;
        // Splice the host-computed per-account state directory
        // into the InitConfig so the adapter can persist its
        // sync cookie + item cache across restarts. EwsAccountConfig
        // reads `state_dir` via serde(default), so absence is fine
        // (test path / older registry builds without data_dir).
        let mut extras: Vec<(&str, Value)> = vec![("password", Value::String(password))];
        let state_dir = self.plugin_state_dir(PLUGIN_ID_EWS, &account.id);
        if let Some(dir) = state_dir.as_ref() {
            extras.push((
                "state_dir",
                Value::String(dir.to_string_lossy().into_owned()),
            ));
        }
        let plugin_config = merge_account_config(&account.config_json, &extras)?;
        let instance = self.open_plugin_instance(PLUGIN_ID_EWS, plugin_config)?;
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_vikunja(&self, account: &Account) -> Result<(), RegistryError> {
        let token = self
            .secret_store
            .retrieve(&account.id, SecretSlot::ApiToken)
            .map_err(|e| RegistryError::Secret(format!("missing API token: {e}")))?;
        let plugin_config =
            merge_account_config(&account.config_json, &[("token", Value::String(token))])?;
        let instance = self.open_plugin_instance(PLUGIN_ID_VIKUNJA, plugin_config)?;
        self.insert_tasks(&account.id, instance)?;
        Ok(())
    }

    fn register_todoist(&self, account: &Account) -> Result<(), RegistryError> {
        // Todoist's persisted config carries only an optional
        // account label; the plugin needs just `token`. We could
        // merge into the existing config but it's cleaner to
        // start fresh — the plugin ignores anything but `token`.
        let token = self
            .secret_store
            .retrieve(&account.id, SecretSlot::ApiToken)
            .map_err(|e| RegistryError::Secret(format!("missing API token: {e}")))?;
        let plugin_config = json!({ "token": token }).to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_TODOIST, plugin_config)?;
        self.insert_tasks(&account.id, instance)?;
        Ok(())
    }

    fn register_ical(&self, account: &Account) -> Result<(), RegistryError> {
        // Basic-auth password is optional for iCal — most public
        // feeds are anonymous. A missing keychain entry is
        // therefore not an error; merge it as JSON null so the
        // plugin's Option<String> deserialiser is happy.
        let password = self
            .secret_store
            .retrieve(&account.id, SecretSlot::Password)
            .ok()
            .filter(|s| !s.is_empty());
        let password_value = password.map(Value::String).unwrap_or(Value::Null);
        let plugin_config =
            merge_account_config(&account.config_json, &[("password", password_value)])?;
        let instance = self.open_plugin_instance(PLUGIN_ID_ICAL, plugin_config)?;
        self.insert_calendar(&account.id, instance)?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // VC-adapter registration (DESIGN.md §11)
    // ─────────────────────────────────────────────────────────────

    fn register_zoom(&self, account: &Account) -> Result<(), RegistryError> {
        let plugin_config = oauth_refresh_plugin_config(
            self.secret_store.as_ref(),
            &account.id,
            &account.config_json,
        )?;
        let instance = self.open_plugin_instance(PLUGIN_ID_ZOOM, plugin_config)?;
        self.insert_vc(&account.id, instance)?;
        Ok(())
    }

    fn register_teams(&self, account: &Account) -> Result<(), RegistryError> {
        // Teams shares the cal-adapter-microsoft-graph access
        // token (same keychain slot it already has). The
        // account row's `config_json` carries just `client_id`;
        // we pull the access token from whichever Graph account
        // is registered + thread it in as `access_token`.
        let access_token = teams_shared_access_token(self.secret_store.as_ref(), &account.id)?;
        let plugin_config = merge_account_config(
            &account.config_json,
            &[("access_token", Value::String(access_token))],
        )?;
        let instance = self.open_plugin_instance(PLUGIN_ID_TEAMS, plugin_config)?;
        self.insert_vc(&account.id, instance)?;
        Ok(())
    }

    fn register_meet(&self, account: &Account) -> Result<(), RegistryError> {
        // Meet shares the cal-adapter-google refresh token
        // (same keychain slot). The account row's `config_json`
        // carries `client_id` + `client_secret`; we pull the
        // refresh token from the linked Google account.
        let refresh_token = self
            .secret_store
            .retrieve(&account.id, SecretSlot::RefreshToken)
            .map_err(|e| RegistryError::Secret(format!("missing refresh token: {e}")))?;
        let plugin_config = merge_account_config(
            &account.config_json,
            &[("refresh_token", Value::String(refresh_token))],
        )?;
        let instance = self.open_plugin_instance(PLUGIN_ID_MEET, plugin_config)?;
        self.insert_vc(&account.id, instance)?;
        Ok(())
    }

    /// Open an instance for any plugin that declared an [`AccountSchema`].
    ///
    /// Everything provider-specific comes out of the schema: which keychain
    /// slots to read, what to call each value in the init config, and whether
    /// there is an OAuth client to resolve. The host contributes only the two
    /// things a plugin cannot do for itself — reaching the platform keychain,
    /// and minting the capability token that lets an instance report a rotated
    /// credential back and be believed about which account it speaks for.
    fn register_from_schema(
        &self,
        account: &Account,
        plugin_id: &str,
        schema: &plugin_core::account_schema::AccountSchema,
    ) -> Result<(), RegistryError> {
        let mut plugin_config =
            crate::account_setup::init_config(schema, &account.config_json, |slot| {
                self.secret_store.retrieve(&account.id, slot)
            })
            .map_err(|e| match e {
                crate::account_setup::AccountSetupError::Config(m)
                | crate::account_setup::AccountSetupError::InvalidInput(m) => {
                    RegistryError::Config(m)
                }
                crate::account_setup::AccountSetupError::Secret(m) => RegistryError::Secret(m),
            })?;

        // The capability token rides the TRANSIENT merged config only — never
        // the persisted row — and must outlive the instance, since dropping it
        // retires the scope. Parking it beside the registration ties the two
        // lifetimes together.
        let scope = schema
            .host_channel
            .then(|| plugin_core::host_channel::mint_scope(&account.id, plugin_id));
        if let Some(scope) = &scope {
            plugin_config = merge_account_config(
                &plugin_config,
                &[(
                    plugin_core::abi::HOST_TOKEN_CONFIG_KEY,
                    Value::String(scope.as_str().to_string()),
                )],
            )?;
        }

        let instance = self.open_plugin_instance(plugin_id, plugin_config)?;
        if let Some(scope) = scope {
            self.retain_scope(&account.id, scope);
        }

        // Which maps the instance lands in follows from the plugin's declared
        // type and capabilities — the same source the per-kind path consults,
        // read generically.
        let manifest = self
            .plugin_manager
            .get(plugin_id)
            .ok_or_else(|| RegistryError::PluginMissing(plugin_id.to_string()))?
            .manifest
            .clone();
        match manifest.plugin_type {
            plugin_core::PluginType::VideoconferenceAdapter => {
                self.insert_vc(&account.id, instance)?;
                // An adapter that can enumerate its meetings also gets a
                // read-only calendar built from them, so meetings created in
                // the provider's own web UI — which have no calendar entry
                // anywhere — become visible. Whether it can is decided by the
                // adapter: `list_meetings` answers Unsupported when the slot is
                // NULL, and then there is simply no such calendar.
                if let Some(vc) = self.vc_adapter(&account.id) {
                    if vc.can_list_meetings() {
                        let calendar = crate::vc_calendar::VcCalendar::new(
                            &account.id,
                            &account.display_name,
                            vc,
                        );
                        self.external_cal
                            .write()
                            .expect("registry cal poison")
                            .insert(account.id.clone(), Arc::new(calendar));
                    }
                }
            }
            plugin_core::PluginType::CalendarAdapter => {
                if manifest.has_capability(&plugin_core::Capability::Calendar) {
                    self.insert_calendar(&account.id, instance.clone())?;
                }
                if manifest.has_capability(&plugin_core::Capability::Tasks) {
                    self.insert_tasks(&account.id, instance.clone())?;
                }
                if manifest.has_capability(&plugin_core::Capability::Contacts) {
                    self.insert_contacts(&account.id, instance)?;
                }
            }
            other => {
                return Err(RegistryError::Construct(format!(
                    "plugin type {other:?} has no account-backed adapter surface"
                )))
            }
        }
        Ok(())
    }
}

/// Merge keychain-sourced secret fields into the account's
/// persisted JSON config + return the resulting string.
///
/// `account_config_json` MUST be a JSON object — every adapter's
/// `AccountConfig` is a struct, so this is always true in
/// practice; non-object payloads are surfaced as
/// `RegistryError::Config`.
///
/// Replaced the per-adapter "parse typed struct → re-encode as
/// plugin JSON" round-trip from the pre-plugin-routing era.
/// Each plugin's `InitConfig` deserialiser carries the field
/// names + types it expects; the registry just needs to make
/// sure the right secret value ends up in the right field. By
/// treating the persisted config as opaque JSON we eliminate
/// the host's compile-time knowledge of each adapter crate's
/// `AccountConfig` shape.
fn merge_account_config(
    account_config_json: &str,
    secrets: &[(&str, Value)],
) -> Result<String, RegistryError> {
    let mut parsed: Value = serde_json::from_str(account_config_json)
        .map_err(|e| RegistryError::Config(e.to_string()))?;
    let obj = parsed.as_object_mut().ok_or_else(|| {
        RegistryError::Config("account config_json must be a JSON object".to_string())
    })?;
    for (key, val) in secrets {
        obj.insert((*key).to_string(), val.clone());
    }
    Ok(parsed.to_string())
}

/// Shared OAuth plugin-config builder for Google + Microsoft
/// Graph. Both plugins expect `access_token` + `refresh_token`
/// + `expires_at` + `scope` on top of whatever their persisted
/// `client_id` / `client_secret` / `authority` config carries.
///
/// `expires_at` is left at epoch — the plugin's API client
/// refreshes lazily on 401, so the persisted access token
/// doesn't need to be fresh across app restarts.
fn oauth_plugin_config(
    secret_store: &dyn SecretStore,
    account_id: &str,
    account_config_json: &str,
) -> Result<String, RegistryError> {
    let access = secret_store
        .retrieve(account_id, SecretSlot::AccessToken)
        .map_err(|e| RegistryError::Secret(format!("missing access token: {e}")))?;
    let refresh = secret_store
        .retrieve(account_id, SecretSlot::RefreshToken)
        .ok()
        .filter(|s| !s.is_empty());
    let refresh_value = refresh.map(Value::String).unwrap_or(Value::Null);

    let mut extras = Map::with_capacity(4);
    extras.insert("access_token".into(), Value::String(access));
    extras.insert("refresh_token".into(), refresh_value);
    extras.insert(
        "expires_at".into(),
        Value::String("1970-01-01T00:00:00Z".into()),
    );
    extras.insert("scope".into(), Value::Null);

    let mut parsed: Value = serde_json::from_str(account_config_json)
        .map_err(|e| RegistryError::Config(e.to_string()))?;
    let obj = parsed.as_object_mut().ok_or_else(|| {
        RegistryError::Config("account config_json must be a JSON object".to_string())
    })?;
    obj.extend(extras);
    Ok(parsed.to_string())
}

/// Build the plugin init config for a refresh-token-only OAuth
/// videoconference adapter (Zoom, WebEx). The persisted
/// `config_json` already carries `client_id` + `client_secret`;
/// we just need to merge in the keychain-sourced
/// `refresh_token`.
fn oauth_refresh_plugin_config(
    secret_store: &dyn SecretStore,
    account_id: &str,
    account_config_json: &str,
) -> Result<String, RegistryError> {
    let refresh = secret_store
        .retrieve(account_id, SecretSlot::RefreshToken)
        .map_err(|e| RegistryError::Secret(format!("missing refresh token: {e}")))?;
    merge_account_config(
        account_config_json,
        &[("refresh_token", Value::String(refresh))],
    )
}

/// Pull the cal-adapter-microsoft-graph access token that the
/// Teams adapter piggybacks on. v1 reads it from the SAME
/// account-id's `AccessToken` slot — the AccountsDialog wizard
/// will create a Teams account whose id maps to its linked
/// Graph calendar account. A later iteration can swap this for
/// a "find the linked Graph account by config_json
/// cross-reference" lookup once the wizard's data model is
/// firm.
fn teams_shared_access_token(
    secret_store: &dyn SecretStore,
    account_id: &str,
) -> Result<String, RegistryError> {
    secret_store
        .retrieve(account_id, SecretSlot::AccessToken)
        .map_err(|e| {
            RegistryError::Secret(format!(
                "missing Microsoft Graph access token (Teams shares it): {e}",
            ))
        })
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("adapter not supported: {0}")]
    Unsupported(String),
    #[error("invalid config: {0}")]
    Config(String),
    #[error("secret missing: {0}")]
    Secret(String),
    #[error("adapter construction failed: {0}")]
    Construct(String),
    /// A credential smoke-test reached the adapter but its read probe failed —
    /// bad auth, an unreachable host, a protocol error. The string is the
    /// underlying adapter error, surfaced to the user.
    #[error("{0}")]
    Probe(String),
    /// The plugin id referenced by the account isn't loaded into
    /// the host's [`PluginManager`] — typically because the user's
    /// other device installed a community plugin Aperio doesn't
    /// have a copy of yet (DESIGN.md §20.8). The UI surfaces this
    /// as a "Plugin fehlt" affordance.
    #[error("plugin not installed: {0}")]
    PluginMissing(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use plugin_core::manifest::PluginManifest;
    use plugin_core::{Capability, PluginType, ABI_VERSION};

    /// Manifest twin for the statically-linked iCal plugin (the dlopen
    /// path reads this from `plugin.json`; a static consumer hands it in).
    fn ical_manifest() -> PluginManifest {
        PluginManifest {
            id: PLUGIN_ID_ICAL.into(),
            name: "Aperio iCal Feed".into(),
            version: "0.1.0".into(),
            plugin_type: PluginType::CalendarAdapter,
            capabilities: vec![Capability::Calendar],
            abi_version: ABI_VERSION,
            min_app_version: "0.1.0".into(),
            author: None,
            description: None,
            signed: false,
            recurrence: Default::default(),
            tasks: Default::default(),
            account: None,
            adapter_kind: None,
        }
    }

    /// Registry backed by exactly one real plugin. iCal needs no keychain
    /// secret and opens its instance without touching the network, so the
    /// register path runs for real without any fixture server.
    fn ical_registry() -> AdapterRegistry {
        let manager = Arc::new(PluginManager::new("0.1.0"));
        let descriptor = unsafe { cal_adapter_ical_plugin::build_descriptor() };
        manager
            .register_static(
                ical_manifest(),
                descriptor,
                cal_adapter_ical_plugin::DESTROY_FN,
            )
            .expect("register the static iCal plugin");
        AdapterRegistry::new(
            manager,
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
        )
    }

    fn ical_config(url: &str) -> String {
        json!({ "feed_url": url, "username": Value::Null }).to_string()
    }

    /// The calendar adapter currently registered for `account_id`, if any.
    fn cal_adapter(
        registry: &AdapterRegistry,
        account_id: &str,
    ) -> Option<Arc<dyn CalendarFeature>> {
        registry
            .snapshot_calendar_adapters()
            .into_iter()
            .find(|(id, _)| id == account_id)
            .map(|(_, adapter)| adapter)
    }

    #[test]
    fn register_missing_registers_only_adapter_less_accounts() {
        let db = DbHandle::open_in_memory().expect("in-memory db");
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let registry = ical_registry();

        // `live` stands in for an account added through the normal command
        // path (already registered); `synced` for one the event-log applier
        // wrote during a sync round — its row exists, its adapter doesn't.
        let live = repo
            .create(
                AdapterKind::new("ical"),
                "Live",
                &ical_config("https://example.invalid/live.ics"),
            )
            .expect("create live account");
        let synced = repo
            .create(
                AdapterKind::new("ical"),
                "Synced",
                &ical_config("https://example.invalid/synced.ics"),
            )
            .expect("create synced account");

        registry.register(&live).expect("register the live account");
        let before = cal_adapter(&registry, &live.id).expect("live adapter present");
        assert!(
            cal_adapter(&registry, &synced.id).is_none(),
            "the synced account starts adapter-less — that IS the bug",
        );

        assert_eq!(
            registry.register_missing(&repo),
            1,
            "only the adapter-less account is registered",
        );

        // The live account's adapter instance must be the SAME Arc: rebuilding
        // it would drop the plugin's in-memory provider state (delta cursors,
        // caches) and force a cold re-drain.
        let after = cal_adapter(&registry, &live.id).expect("live adapter still present");
        assert!(
            Arc::ptr_eq(&before, &after),
            "an already-registered account must not be rebuilt",
        );
        assert!(
            cal_adapter(&registry, &synced.id).is_some(),
            "the synced account now has an adapter",
        );

        // Idempotent: a second round applies nothing new, so nothing registers.
        assert_eq!(registry.register_missing(&repo), 0);
    }

    #[test]
    fn register_missing_skips_local_and_device_calendar_accounts() {
        let db = DbHandle::open_in_memory().expect("in-memory db");
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let registry = ical_registry();

        // Migration 0003 seeds the implicit local account; the device
        // calendar is built by the cal-ffi layer against its native bridge.
        // Neither may be registered here — and neither counts as "newly
        // registered", so a round with only these must not kick a warm pass.
        repo.create(AdapterKind::new("device_calendar"), "Phone", "{}")
            .expect("create device-calendar account");
        assert_eq!(registry.register_missing(&repo), 0);
        assert!(cal_adapter(&registry, LOCAL_ID).is_none());
    }
}
