//! Exchange Web Services (EWS) adapter — Phase 6f.1a (read-only).
//!
//! EWS is the SOAP-over-HTTP API Microsoft shipped for on-premise
//! Exchange servers and a handful of Exchange-alike products
//! (Kerio Connect, Zimbra-with-EWS-plugin, …). It's the lowest-
//! common-denominator API in the Microsoft ecosystem — once Exchange
//! Online started pushing customers towards Graph the EWS surface
//! stopped growing, but on-premise installs still rely on it as
//! their default external interface.
//!
//! Phase 6f.1a scope:
//!
//!   - manual server URL (no `Autodiscover.svc` resolution yet)
//!   - Basic auth (no NTLM, no OAuth-against-EWS for Online)
//!   - `FindFolder` over `msgfolderroot` restricted to
//!     `IPF.Appointment` → calendar list
//!   - `FindItem` with `CalendarView` → events in a bounded window
//!   - 5-minute listing-cache TTL, mirroring CalDAV / Google / Graph
//!   - all write methods are `Unsupported`; Phase 6f.1b adds them
//!
//! `cal_core::Calendar.read_only` is `true` for every calendar listed
//! here so the UI doesn't expose an Edit button on something we can't
//! yet PUT back. The user-visible name override + rename flow still
//! works via the local-override fallback that the command layer
//! provides when `rename_calendar` returns `Unsupported`.

pub mod api;
pub mod auth;
pub mod autodiscover;
pub mod error;
pub mod mapping;
pub mod soap;
pub mod tasks;

use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, ContainerColor,
    Credentials as CoreCredentials, DateRange, Error as CoreError, Event, FreeBusy,
    NewEvent, NewTask, Result as CoreResult, Task, TaskList, TasksFeature,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub use auth::BasicCredentials;
pub use autodiscover::{discover, discover_client, DiscoveredEndpoints};
pub use error::{EwsError, EwsResult};

use crate::api::EwsClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EwsAccountConfig {
    /// Endpoint URL, e.g. `https://mail.example.org/EWS/Exchange.asmx`.
    /// Aperio asks the user for this directly — autodiscover comes
    /// later (Phase 6f.2).
    pub endpoint: String,
    pub username: String,
    #[serde(default)]
    pub account_label: Option<String>,
}

#[derive(Debug)]
pub struct EwsAdapter {
    client: EwsClient,
    capabilities: Vec<Capability>,
    calendars_cache: Mutex<Option<(Vec<Calendar>, chrono::DateTime<chrono::Utc>)>>,
    task_lists_cache: Mutex<Option<(Vec<TaskList>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl EwsAdapter {
    pub fn new(endpoint: String, credentials: BasicCredentials) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            // Some on-premise Exchange installs sit behind an
            // SSL-terminating proxy that issues a 301 to the
            // canonicalised host; reqwest follows up to 10 redirects
            // by default which is plenty.
            .build()
            .expect("reqwest client");
        Self {
            client: EwsClient::new(endpoint, credentials, http),
            capabilities: vec![Capability::Calendar, Capability::Tasks],
            calendars_cache: Mutex::new(None),
            task_lists_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl = chrono::Duration::from_std(ttl)
            .unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    async fn cached_calendars(&self) -> Option<Vec<Calendar>> {
        let guard = self.calendars_cache.lock().await;
        let (items, ts) = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(*ts);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(items.clone())
        } else {
            None
        }
    }

    async fn cached_task_lists(&self) -> Option<Vec<TaskList>> {
        let guard = self.task_lists_cache.lock().await;
        let (items, ts) = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(*ts);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(items.clone())
        } else {
            None
        }
    }
}

#[async_trait]
impl Adapter for EwsAdapter {
    async fn authenticate(
        &self,
        _credentials: CoreCredentials,
    ) -> CoreResult<AuthToken> {
        // EWS uses Basic auth carried per-request; there's no
        // separate auth step the way OAuth has. We return an empty
        // token so the trait stays satisfied, and the registry never
        // persists anything beyond the keychain-stored password.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for EwsAdapter {
    async fn list_calendars(&self) -> CoreResult<Vec<Calendar>> {
        if let Some(cached) = self.cached_calendars().await {
            return Ok(cached);
        }
        let fresh = api::list_calendars(&self.client)
            .await
            .map_err(to_core_error)?;
        *self.calendars_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_events(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> CoreResult<Vec<Event>> {
        api::get_events(&self.client, calendar_id, range.start, range.end)
            .await
            .map_err(to_core_error)
    }

    async fn create_event(
        &self,
        calendar_id: &str,
        event: NewEvent,
    ) -> CoreResult<Event> {
        api::create_event(&self.client, calendar_id, event)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        api::update_event(&self.client, &event)
            .await
            .map_err(to_core_error)
    }

    async fn delete_event(&self, event_id: &str) -> CoreResult<()> {
        api::delete_event(&self.client, event_id)
            .await
            .map_err(to_core_error)
    }

    async fn rename_calendar(
        &self,
        calendar_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        api::rename_calendar(&self.client, calendar_id, new_name)
            .await
            .map_err(to_core_error)?;
        // Drop the cached calendar list so the next list_calendars
        // round-trip picks up the new display name. The ChangeKey
        // advances server-side too — we don't bother to harvest the
        // new one here because subsequent reads will get the fresh
        // pair via FindFolder anyway.
        *self.calendars_cache.lock().await = None;
        Ok(())
    }

    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> CoreResult<Vec<FreeBusy>> {
        // EWS does expose GetUserAvailability — wiring it up costs
        // another envelope + parser pair, deferred to a later phase.
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // EWS doesn't surface a per-folder colour on the wire; the
        // colour lives in the user's Outlook profile. Leave `None`
        // and let the local override system supply a colour if the
        // user wants one.
        None
    }

    /// EWS doesn't model EXDATE as an editable property on the master
    /// — instead, the server-side equivalent is `DeleteItem` against
    /// the specific occurrence id, which removes that one date from
    /// future expansions. The dedicated `api::add_event_exdate` takes
    /// the raw decoded id (without master-resolution) so this stays
    /// per-occurrence; the supplied `occurrence` datetime is
    /// redundant because the ItemId already uniquely identifies the
    /// row.
    async fn add_event_exdate(
        &self,
        event_id: &str,
        _occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        api::add_event_exdate(&self.client, event_id)
            .await
            .map_err(to_core_error)
    }
}

#[async_trait]
impl TasksFeature for EwsAdapter {
    async fn list_task_lists(&self) -> CoreResult<Vec<TaskList>> {
        if let Some(cached) = self.cached_task_lists().await {
            return Ok(cached);
        }
        let fresh = tasks::list_task_lists(&self.client)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        tasks::get_tasks(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(
        &self,
        list_id: &str,
        task: NewTask,
    ) -> CoreResult<Task> {
        tasks::create_task(&self.client, list_id, task)
            .await
            .map_err(to_core_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        tasks::update_task(&self.client, &task)
            .await
            .map_err(to_core_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        tasks::delete_task(&self.client, task_id)
            .await
            .map_err(to_core_error)
    }

    async fn rename_task_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        tasks::rename_task_list(&self.client, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        // Same cache invalidation pattern as `rename_calendar`: drop
        // the cached list so the next list_task_lists round-trip
        // picks up the new display name.
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }
}

fn to_core_error(err: EwsError) -> CoreError {
    use EwsError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("EWS HTTP {status}: {message}")),
        },
        Soap { code, message } => match code.as_str() {
            // EWS encodes auth + permission failures in the SOAP body
            // even though the HTTP status is 200. Route the familiar
            // codes into the matching cal-core variants so the UI can
            // present "wrong password" specifically rather than a
            // generic protocol error.
            "ErrorAccessDenied" | "ErrorInvalidAccessToken"
            | "ErrorPasswordExpired" | "ErrorADUnavailable"
            | "ErrorNoFreeBusyAccess" => CoreError::Authentication(message),
            "ErrorItemNotFound" | "ErrorFolderNotFound" => CoreError::NotFound(message),
            _ => CoreError::Protocol(format!("EWS SOAP {code}: {message}")),
        },
        Protocol(m) => CoreError::Protocol(m),
        Config(m) => CoreError::InvalidInput(m),
        DiscoveryFailed(m) => CoreError::NotFound(m),
    }
}
