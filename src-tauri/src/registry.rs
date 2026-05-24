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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use cal_adapter_caldav::{
    config::{CaldavAccountConfig, Credentials as CaldavCredentials},
    CaldavAdapter,
};
use cal_adapter_ews::{BasicCredentials as EwsCredentials, EwsAccountConfig, EwsAdapter};
use cal_adapter_google::{GoogleAccountConfig, GoogleAdapter, TokenSet as GoogleTokenSet};
use cal_adapter_todoist::{TodoistAccountConfig, TodoistAdapter};
use cal_adapter_vikunja::{VikunjaAccountConfig, VikunjaAdapter};
use cal_adapter_ical::{
    Credentials as IcalCredentials, IcalAccountConfig, IcalAdapter,
};
use cal_adapter_microsoft_graph::{
    GraphAccountConfig, MicrosoftGraphAdapter, TokenSet as GraphTokenSet,
};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use tracing::warn;

use crate::accounts::{Account, AccountsRepo, AdapterKind, LOCAL_ACCOUNT_ID};
use crate::secrets::{self, SecretSlot};

/// Account-id used for the implicit local adapter. Mirrors the value
/// the `accounts` table seeds during migration 0003.
pub const LOCAL_ID: &str = LOCAL_ACCOUNT_ID;

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
    /// Empty in Phase 10a — Aperio's three CardDAV-capable adapters
    /// (CalDAV/CardDAV, Google People, MS Graph Contacts) grow the
    /// feature in 10b. The slot exists now so the registry surface
    /// and the routing helpers don't have to be redesigned when
    /// they land.
    external_contacts: RwLock<HashMap<String, Arc<dyn ContactsFeature>>>,
    /// Reverse lookup for routing writes back to the right adapter.
    routes: Mutex<Routes>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self {
            external_cal: RwLock::new(HashMap::new()),
            external_tasks: RwLock::new(HashMap::new()),
            external_contacts: RwLock::new(HashMap::new()),
            routes: Mutex::new(Routes::default()),
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

    /// Counterpart of `snapshot_calendar_adapters` for the tasks side.
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

    /// Counterpart of `snapshot_calendar_adapters` for the contacts
    /// side. Currently always empty until Phase 10b lights up the
    /// CardDAV adapter; the method exists so the aggregation
    /// helpers (`list_external_contact_lists`,
    /// `search_external_contacts`) compile without conditional
    /// guards.
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

    /// Aggregate `list_contact_lists` across every external
    /// adapter that declares `ContactsFeature`. Errors per account
    /// are logged and skipped — same shape as the calendar / tasks
    /// equivalents.
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

    /// Fan `search_contacts` out across every external adapter and
    /// concatenate the hits. The local adapter is searched
    /// separately by the command layer because it isn't part of
    /// this registry. Adapters that fail are logged; their
    /// absence shouldn't block hits from the rest.
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

    /// After the registry routes a write back to a specific
    /// account, this returns the adapter handle. Returns `None`
    /// when the account is unknown — the caller maps that to a
    /// `not_found` error so the frontend can show a clear message.
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
        let credentials = CaldavCredentials::new(config, secret);
        let adapter = CaldavAdapter::new(credentials, None)
            .map_err(|e| RegistryError::Construct(e.to_string()))?;
        let arc = Arc::new(adapter);
        // CalDAV adapters speak three feature traits now (Phase 10b
        // light up the contacts side). All three slots route to the
        // same `Arc<CaldavAdapter>` — discovery, listing caches and
        // the HTTP client are shared. Servers without CardDAV
        // surface a no-op `list_contact_lists` and the registry's
        // aggregation skips them.
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(
                account.id.clone(),
                arc.clone() as Arc<dyn CalendarFeature>,
            );
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(account.id.clone(), arc.clone() as Arc<dyn TasksFeature>);
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .insert(account.id.clone(), arc as Arc<dyn ContactsFeature>);
        Ok(())
    }

    /// Wire up a Google Calendar account. Tokens come out of the
    /// platform keychain (stored after the OAuth dance by the
    /// `connect_google_account` command). `expires_at` is left at
    /// epoch — the adapter's API layer refreshes lazily on the
    /// first 401, so the persisted access token doesn't need to be
    /// fresh across app restarts.
    fn register_google(&self, account: &Account) -> Result<(), RegistryError> {
        let config: GoogleAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let access = secrets::retrieve(&account.id, SecretSlot::AccessToken)
            .map_err(|e| RegistryError::Secret(format!("missing access token: {e}")))?;
        let refresh = secrets::retrieve(&account.id, SecretSlot::RefreshToken)
            .ok()
            .filter(|s| !s.is_empty());
        let tokens = GoogleTokenSet {
            access_token: access,
            refresh_token: refresh,
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)
                .unwrap_or_else(chrono::Utc::now),
            scope: None,
        };
        let adapter = GoogleAdapter::new(config.client_id, config.client_secret, tokens);
        let arc = Arc::new(adapter);
        // Phase 6d.3 + 10h: the same adapter instance serves all
        // three feature traits. The combined OAuth scope (see
        // auth::SCOPES) gives a single access token rights over
        // Calendar + Tasks + Contacts, so the shared
        // `Arc<GoogleAdapter>` keeps the in-memory token + listing
        // caches coherent across every read path.
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(
                account.id.clone(),
                arc.clone() as Arc<dyn CalendarFeature>,
            );
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(account.id.clone(), arc.clone() as Arc<dyn TasksFeature>);
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .insert(account.id.clone(), arc as Arc<dyn ContactsFeature>);
        Ok(())
    }

    /// Wire up a Microsoft Graph (Outlook) account. Mirrors the
    /// Google flow: tokens come out of the keychain, the API
    /// layer's lazy 401-refresh restores a fresh access token if
    /// the stored one expired between sessions.
    fn register_microsoft_graph(&self, account: &Account) -> Result<(), RegistryError> {
        let config: GraphAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let access = secrets::retrieve(&account.id, SecretSlot::AccessToken)
            .map_err(|e| RegistryError::Secret(format!("missing access token: {e}")))?;
        let refresh = secrets::retrieve(&account.id, SecretSlot::RefreshToken)
            .ok()
            .filter(|s| !s.is_empty());
        let tokens = GraphTokenSet {
            access_token: access,
            refresh_token: refresh,
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)
                .unwrap_or_else(chrono::Utc::now),
            scope: None,
        };
        let adapter = MicrosoftGraphAdapter::new(
            config.client_id,
            config.authority,
            tokens,
        );
        let arc = Arc::new(adapter);
        // Phase 6e.1 + 6e.2 + 10i: the Graph adapter declares
        // Calendar + Tasks + Contacts. Same
        // `Arc<MicrosoftGraphAdapter>` instance is reused across
        // every feature trait so the shared OAuth token state +
        // listing caches stay coherent across reads.
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(
                account.id.clone(),
                arc.clone() as Arc<dyn CalendarFeature>,
            );
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(account.id.clone(), arc.clone() as Arc<dyn TasksFeature>);
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .insert(account.id.clone(), arc as Arc<dyn ContactsFeature>);
        Ok(())
    }

    /// Wire up an EWS (Exchange Web Services) account. Basic-auth-
    /// only for the first cut — the keychain holds the password,
    /// the JSON config holds the endpoint URL + username. All three
    /// feature surfaces are registered against the same EwsAdapter
    /// instance so the per-account listing caches stay coherent:
    /// Phase 6f.1 wired up calendars, 6f.2 added tasks, 10e added
    /// contacts (`IPF.Contact` folders + `<t:Contact>` items).
    fn register_ews(&self, account: &Account) -> Result<(), RegistryError> {
        let config: EwsAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let password = secrets::retrieve(&account.id, SecretSlot::Password)
            .map_err(|e| RegistryError::Secret(format!("missing password: {e}")))?;
        let credentials = EwsCredentials {
            username: config.username,
            password,
        };
        let adapter = EwsAdapter::new(config.endpoint, credentials);
        let arc = Arc::new(adapter);
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(
                account.id.clone(),
                arc.clone() as Arc<dyn CalendarFeature>,
            );
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(
                account.id.clone(),
                arc.clone() as Arc<dyn TasksFeature>,
            );
        // Phase 10e: the same EwsAdapter also serves ContactsFeature
        // against the user's `IPF.Contact` folders. The per-account
        // listing caches stay coherent across all three feature
        // traits because they share the same Arc.
        self.external_contacts
            .write()
            .expect("registry contacts poison")
            .insert(account.id.clone(), arc as Arc<dyn ContactsFeature>);
        Ok(())
    }

    /// Wire up a Vikunja account. Vikunja is tasks-only — we register
    /// the adapter under `external_tasks` and skip `external_cal`.
    /// The API token comes out of the platform keychain under
    /// `SecretSlot::ApiToken` (the slot is named for exactly this
    /// use case — long-lived third-party-client tokens).
    fn register_vikunja(&self, account: &Account) -> Result<(), RegistryError> {
        let config: VikunjaAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        let token = secrets::retrieve(&account.id, SecretSlot::ApiToken)
            .map_err(|e| RegistryError::Secret(format!("missing API token: {e}")))?;
        let adapter = VikunjaAdapter::new(&config.server_url, token)
            .map_err(|e| RegistryError::Construct(e.to_string()))?;
        let arc = Arc::new(adapter);
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(account.id.clone(), arc as Arc<dyn TasksFeature>);
        Ok(())
    }

    /// Wire up a Todoist account. Same shape as Vikunja: tasks-only,
    /// API token in the keychain under `SecretSlot::ApiToken`. The
    /// config carries no server URL — Todoist is hosted and the
    /// base URL is hard-coded in the adapter.
    fn register_todoist(&self, account: &Account) -> Result<(), RegistryError> {
        // Empty `config_json` is allowed — the `TodoistAccountConfig`
        // only holds an optional account label and Todoist itself
        // doesn't need anything but the token. Parse with defaults so
        // older accounts written as `{}` still work.
        let _config: TodoistAccountConfig =
            serde_json::from_str(&account.config_json)
                .unwrap_or_default();
        let token = secrets::retrieve(&account.id, SecretSlot::ApiToken)
            .map_err(|e| RegistryError::Secret(format!("missing API token: {e}")))?;
        let adapter = TodoistAdapter::new(token);
        let arc = Arc::new(adapter);
        self.external_tasks
            .write()
            .expect("registry tasks poison")
            .insert(account.id.clone(), arc as Arc<dyn TasksFeature>);
        Ok(())
    }

    /// Wire up an iCal feed account. Only the calendar side is
    /// registered; iCal feeds don't carry VTODOs in a queryable way,
    /// so we skip the TasksFeature slot rather than expose a row that
    /// would always be empty.
    fn register_ical(&self, account: &Account) -> Result<(), RegistryError> {
        let config: IcalAccountConfig = serde_json::from_str(&account.config_json)
            .map_err(|e| RegistryError::Config(e.to_string()))?;
        // Basic-auth password is optional for iCal — most public feeds
        // are anonymous. A missing keychain entry is therefore not an
        // error; an explicit empty string from the secrets store
        // collapses to None too.
        let password = secrets::retrieve(&account.id, SecretSlot::Password)
            .ok()
            .filter(|s| !s.is_empty());
        let credentials = IcalCredentials::new(config, password);
        let adapter = IcalAdapter::new(credentials)
            .map_err(|e| RegistryError::Construct(e.to_string()))?;
        let arc = Arc::new(adapter);
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(account.id.clone(), arc as Arc<dyn CalendarFeature>);
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
}
