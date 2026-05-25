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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use cal_adapter_caldav::{
    config::{CaldavAccountConfig, AuthKind as CaldavAuthKind},
};
use cal_adapter_ews::EwsAccountConfig;
use cal_adapter_google::GoogleAccountConfig;
use cal_adapter_todoist::TodoistAccountConfig;
use cal_adapter_vikunja::VikunjaAccountConfig;
use cal_adapter_ical::IcalAccountConfig;
use cal_adapter_microsoft_graph::GraphAccountConfig;
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use plugin_core::shim::{FfiCalendarAdapter, FfiContactsAdapter, FfiTasksAdapter};
use plugin_core::{LoadedInstance, LoadedPlugin, PluginManager};
use tracing::warn;

use crate::accounts::{Account, AccountsRepo, AdapterKind, LOCAL_ACCOUNT_ID};
use crate::secrets::{self, SecretSlot};

/// Account-id used for the implicit local adapter. Mirrors the value
/// the `accounts` table seeds during migration 0003.
pub const LOCAL_ID: &str = LOCAL_ACCOUNT_ID;

/// Plugin-id constants — the strings the bundled plugins advertise
/// in their `aperio_plugin_create` descriptor + their `plugin.json`
/// manifests. Centralised here so the per-adapter routing matches
/// each plugin verbatim.
const PLUGIN_ID_CALDAV: &str = "com.aperio.cal-adapter-caldav";
const PLUGIN_ID_ICAL: &str = "com.aperio.cal-adapter-ical";
const PLUGIN_ID_GOOGLE: &str = "com.aperio.cal-adapter-google";
const PLUGIN_ID_GRAPH: &str = "com.aperio.cal-adapter-microsoft-graph";
const PLUGIN_ID_EWS: &str = "com.aperio.cal-adapter-ews";
const PLUGIN_ID_VIKUNJA: &str = "com.aperio.cal-adapter-vikunja";
const PLUGIN_ID_TODOIST: &str = "com.aperio.cal-adapter-todoist";

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
    /// Reverse lookup for routing writes back to the right adapter.
    routes: Mutex<Routes>,
    /// Loaded plugins keyed by their canonical id. Every
    /// `register_*` fn pulls the matching plugin out of here +
    /// opens a fresh per-account instance against it. Empty on
    /// the test path (in which case `register_*` calls fail
    /// cleanly with `RegistryError::PluginMissing`).
    plugin_manager: Arc<PluginManager>,
}

impl AdapterRegistry {
    /// Construct a registry against the host's plugin manager.
    /// Production callers pass the `Arc<PluginManager>` built at
    /// app startup (after `register_static` ran for every
    /// bundled plugin); tests can pass an empty
    /// `PluginManager::new("0.1.0")` when they don't exercise the
    /// register / bootstrap path.
    pub fn new(plugin_manager: Arc<PluginManager>) -> Self {
        Self {
            external_cal: RwLock::new(HashMap::new()),
            external_tasks: RwLock::new(HashMap::new()),
            external_contacts: RwLock::new(HashMap::new()),
            routes: Mutex::new(Routes::default()),
            plugin_manager,
        }
    }

    /// Build adapters for every persisted external account. Failures
    /// per account are logged and skipped so one broken row doesn't
    /// stop the rest of the app from booting.
    pub fn bootstrap(&self, repo: &AccountsRepo<'_>) {
        let accounts = match repo.list() {
            Ok(a) => a,
            Err(err) => {
                warn!(?err, "failed to list accounts at bootstrap");
                return;
            }
        };
        for account in accounts {
            if account.adapter_kind == AdapterKind::Local {
                continue;
            }
            if let Err(err) = self.try_register(&account) {
                warn!(
                    account_id = %account.id,
                    kind = ?account.adapter_kind,
                    ?err,
                    "skipping account at bootstrap"
                );
            }
        }
    }

    /// Register a single account at runtime. Used by
    /// `create_account` after a successful authentication smoke
    /// test so the new adapter becomes routable immediately
    /// without an app restart.
    pub fn register(&self, account: &Account) -> Result<(), RegistryError> {
        self.try_register(account)
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
    pub fn snapshot_calendar_adapters(
        &self,
    ) -> Vec<(String, Arc<dyn CalendarFeature>)> {
        self.external_cal
            .read()
            .expect("registry cal poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn snapshot_task_adapters(
        &self,
    ) -> Vec<(String, Arc<dyn TasksFeature>)> {
        self.external_tasks
            .read()
            .expect("registry tasks poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn snapshot_contact_adapters(
        &self,
    ) -> Vec<(String, Arc<dyn ContactsFeature>)> {
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
        let mut out = Vec::new();
        for (account_id, adapter) in snapshot {
            match adapter.list_calendars().await {
                Ok(cals) => {
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

    pub async fn search_external_contacts(
        &self,
        query: &str,
    ) -> Vec<cal_core::Contact> {
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

    pub fn calendar_adapter(
        &self,
        account_id: &str,
    ) -> Option<Arc<dyn CalendarFeature>> {
        self.external_cal
            .read()
            .expect("registry cal poison")
            .get(account_id)
            .cloned()
    }

    pub fn task_adapter(
        &self,
        account_id: &str,
    ) -> Option<Arc<dyn TasksFeature>> {
        self.external_tasks
            .read()
            .expect("registry tasks poison")
            .get(account_id)
            .cloned()
    }

    pub fn contact_adapter(
        &self,
        account_id: &str,
    ) -> Option<Arc<dyn ContactsFeature>> {
        self.external_contacts
            .read()
            .expect("registry contacts poison")
            .get(account_id)
            .cloned()
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
        self.plugin_manager.get(plugin_id).ok_or_else(|| {
            RegistryError::PluginMissing(plugin_id.to_string())
        })
    }

    /// Open an instance of the named plugin with the supplied JSON
    /// config. Maps plugin-side errors into the registry error
    /// type so callers can handle them uniformly.
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
            RegistryError::Construct(
                "plugin doesn't expose the CalendarFeature surface".into(),
            )
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
            RegistryError::Construct(
                "plugin doesn't expose the TasksFeature surface".into(),
            )
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
            RegistryError::Construct(
                "plugin doesn't expose the ContactsFeature surface".into(),
            )
        })?;
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .insert(account_id.to_string(), Arc::new(adapter));
        Ok(())
    }

    fn try_register(&self, account: &Account) -> Result<(), RegistryError> {
        match account.adapter_kind {
            AdapterKind::Local => Ok(()),
            AdapterKind::Caldav => self.register_caldav(account),
            AdapterKind::Ical => self.register_ical(account),
            AdapterKind::Google => self.register_google(account),
            AdapterKind::MicrosoftGraph => self.register_microsoft_graph(account),
            AdapterKind::Ews => self.register_ews(account),
            AdapterKind::Vikunja => self.register_vikunja(account),
            AdapterKind::Todoist => self.register_todoist(account),
        }
    }

    fn register_caldav(&self, account: &Account) -> Result<(), RegistryError> {
        let config: CaldavAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let secret = secrets::retrieve(&account.id, SecretSlot::Password)
            .map_err(|e| RegistryError::Secret(format!("missing password: {e}")))?;
        // CalDAV's auth_kind is the snake-case-serialising AuthKind enum;
        // the plugin's InitConfig deserialises it back through the same
        // serde shape, so we just round-trip via json! { "auth_kind": ... }.
        let auth_kind = match config.auth_kind {
            CaldavAuthKind::Basic => "basic",
            CaldavAuthKind::Bearer => "bearer",
        };
        let plugin_config = serde_json::json!({
            "server_url": config.server_url,
            "username": config.username,
            "auth_kind": auth_kind,
            "secret": secret,
        })
        .to_string();
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
        let config: GoogleAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let access = secrets::retrieve(&account.id, SecretSlot::AccessToken)
            .map_err(|e| RegistryError::Secret(format!("missing access token: {e}")))?;
        let refresh = secrets::retrieve(&account.id, SecretSlot::RefreshToken)
            .ok()
            .filter(|s| !s.is_empty());
        // expires_at is left at epoch — the plugin's API client
        // refreshes lazily on 401 so the persisted access token
        // doesn't need to be fresh across app restarts.
        let plugin_config = serde_json::json!({
            "client_id": config.client_id,
            "client_secret": config.client_secret,
            "access_token": access,
            "refresh_token": refresh,
            "expires_at": "1970-01-01T00:00:00Z",
            "scope": null,
        })
        .to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_GOOGLE, plugin_config)?;
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_microsoft_graph(&self, account: &Account) -> Result<(), RegistryError> {
        let config: GraphAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let access = secrets::retrieve(&account.id, SecretSlot::AccessToken)
            .map_err(|e| RegistryError::Secret(format!("missing access token: {e}")))?;
        let refresh = secrets::retrieve(&account.id, SecretSlot::RefreshToken)
            .ok()
            .filter(|s| !s.is_empty());
        let plugin_config = serde_json::json!({
            "client_id": config.client_id,
            "authority": config.authority,
            "access_token": access,
            "refresh_token": refresh,
            "expires_at": "1970-01-01T00:00:00Z",
            "scope": null,
        })
        .to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_GRAPH, plugin_config)?;
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_ews(&self, account: &Account) -> Result<(), RegistryError> {
        let config: EwsAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let password = secrets::retrieve(&account.id, SecretSlot::Password)
            .map_err(|e| RegistryError::Secret(format!("missing password: {e}")))?;
        let plugin_config = serde_json::json!({
            "endpoint": config.endpoint,
            "username": config.username,
            "password": password,
        })
        .to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_EWS, plugin_config)?;
        self.insert_calendar(&account.id, instance.clone())?;
        self.insert_tasks(&account.id, instance.clone())?;
        self.insert_contacts(&account.id, instance)?;
        Ok(())
    }

    fn register_vikunja(&self, account: &Account) -> Result<(), RegistryError> {
        let config: VikunjaAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let token = secrets::retrieve(&account.id, SecretSlot::ApiToken)
            .map_err(|e| RegistryError::Secret(format!("missing API token: {e}")))?;
        let plugin_config = serde_json::json!({
            "server_url": config.server_url,
            "token": token,
        })
        .to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_VIKUNJA, plugin_config)?;
        self.insert_tasks(&account.id, instance)?;
        Ok(())
    }

    fn register_todoist(&self, account: &Account) -> Result<(), RegistryError> {
        // Empty `config_json` is allowed — the TodoistAccountConfig
        // only holds an optional account label and Todoist itself
        // doesn't need anything but the token. Parse with defaults
        // so older accounts written as `{}` still work.
        let _config: TodoistAccountConfig =
            serde_json::from_str(&account.config_json).unwrap_or_default();
        let token = secrets::retrieve(&account.id, SecretSlot::ApiToken)
            .map_err(|e| RegistryError::Secret(format!("missing API token: {e}")))?;
        let plugin_config = serde_json::json!({ "token": token }).to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_TODOIST, plugin_config)?;
        self.insert_tasks(&account.id, instance)?;
        Ok(())
    }

    fn register_ical(&self, account: &Account) -> Result<(), RegistryError> {
        let config: IcalAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        // Basic-auth password is optional for iCal — most public
        // feeds are anonymous. A missing keychain entry is
        // therefore not an error.
        let password = secrets::retrieve(&account.id, SecretSlot::Password)
            .ok()
            .filter(|s| !s.is_empty());
        let plugin_config = serde_json::json!({
            "feed_url": config.feed_url,
            "username": config.username,
            "password": password,
        })
        .to_string();
        let instance = self.open_plugin_instance(PLUGIN_ID_ICAL, plugin_config)?;
        self.insert_calendar(&account.id, instance)?;
        Ok(())
    }
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
    /// The plugin id referenced by the account isn't loaded into
    /// the host's [`PluginManager`] — typically because the user's
    /// other device installed a community plugin Aperio doesn't
    /// have a copy of yet (DESIGN.md §20.8). The UI surfaces this
    /// as a "Plugin fehlt" affordance.
    #[error("plugin not installed: {0}")]
    PluginMissing(String),
}
