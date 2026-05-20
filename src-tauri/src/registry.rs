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
use cal_adapter_ical::{
    Credentials as IcalCredentials, IcalAccountConfig, IcalAdapter,
};
use cal_core::{CalendarFeature, TasksFeature};
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
}

/// Process-wide registry of all non-local adapter instances.
pub struct AdapterRegistry {
    /// External adapters with CalendarFeature, keyed by account_id.
    external_cal: RwLock<HashMap<String, Arc<dyn CalendarFeature>>>,
    /// External adapters with TasksFeature, keyed by account_id.
    external_tasks: RwLock<HashMap<String, Arc<dyn TasksFeature>>>,
    /// Reverse lookup for routing writes back to the right adapter.
    routes: Mutex<Routes>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self {
            external_cal: RwLock::new(HashMap::new()),
            external_tasks: RwLock::new(HashMap::new()),
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
        let mut routes = self.routes.lock().expect("registry routes poison");
        routes
            .calendar_to_account
            .retain(|_, owner| owner != account_id);
        routes
            .list_to_account
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

    /// Run `list_calendars` on every external CalendarFeature
    /// adapter and return the flat aggregated list. Errors from
    /// one adapter don't poison the rest; they are logged and the
    /// other accounts still get to show up.
    pub async fn list_external_calendars(&self) -> Vec<cal_core::Calendar> {
        let snapshot: Vec<(String, Arc<dyn CalendarFeature>)> = self
            .external_cal
            .read()
            .expect("registry cal poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
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
        let snapshot: Vec<(String, Arc<dyn TasksFeature>)> = self
            .external_tasks
            .read()
            .expect("registry tasks poison")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
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

    fn try_register(&self, account: &Account) -> Result<(), RegistryError> {
        match account.adapter_kind {
            AdapterKind::Local => Ok(()),
            AdapterKind::Caldav => self.register_caldav(account),
            AdapterKind::Ical => self.register_ical(account),
            other => Err(RegistryError::Unsupported(format!(
                "adapter kind '{}' is not wired up yet",
                other.as_str()
            ))),
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
        self.external_cal
            .write()
            .expect("registry cal poison")
            .insert(account.id.clone(), arc.clone() as Arc<dyn CalendarFeature>);
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
