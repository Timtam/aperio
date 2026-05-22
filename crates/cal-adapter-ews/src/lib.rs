//! Exchange Web Services (EWS) adapter.
//!
//! EWS is the SOAP-over-HTTP API Microsoft shipped for on-premise
//! Exchange servers and a handful of Exchange-alike products
//! (Kerio Connect, Zimbra-with-EWS-plugin, …). It's the lowest-
//! common-denominator API in the Microsoft ecosystem — once Exchange
//! Online started pushing customers towards Graph the EWS surface
//! stopped growing, but on-premise installs still rely on it as
//! their default external interface.
//!
//! Feature surface (built up incrementally across phases):
//!
//!   - **Calendar** (Phase 6f.1a/1b/1c): `IPF.Appointment` folders,
//!     `CalendarView` for date-windowed events, full CRUD including
//!     recurring-master series editing.
//!   - **Tasks** (Phase 6f.2): `IPF.Task` folders, `<t:Task>` items,
//!     full CRUD without recurrence (recurring tasks are out of
//!     scope for the first cut).
//!   - **Contacts** (Phase 10e): `IPF.Contact` folders,
//!     `<t:Contact>` items, full CRUD with indexed property
//!     handling for emails / phone numbers and a client-side
//!     fan-out for cross-list search.
//!
//! Auth + transport:
//!
//!   - Manual server URL (no `Autodiscover.svc` resolution yet)
//!   - Basic auth (no NTLM, no OAuth-against-EWS for Online)
//!   - 5-minute listing-cache TTL across all three feature surfaces,
//!     mirroring CalDAV / Google / Graph

pub mod api;
pub mod auth;
pub mod autodiscover;
pub mod contacts;
pub mod error;
pub mod mapping;
pub mod soap;
pub mod tasks;

use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, Contact, ContactList,
    ContactsFeature, ContainerColor, Credentials as CoreCredentials, DateRange,
    Error as CoreError, Event, FreeBusy, NewContact, NewEvent, NewTask,
    Result as CoreResult, Task, TaskList, TasksFeature,
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
    contact_lists_cache:
        Mutex<Option<(Vec<ContactList>, chrono::DateTime<chrono::Utc>)>>,
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
            capabilities: vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts,
            ],
            calendars_cache: Mutex::new(None),
            task_lists_cache: Mutex::new(None),
            contact_lists_cache: Mutex::new(None),
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

    async fn cached_contact_lists(&self) -> Option<Vec<ContactList>> {
        let guard = self.contact_lists_cache.lock().await;
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

#[async_trait]
impl ContactsFeature for EwsAdapter {
    async fn list_contact_lists(&self) -> CoreResult<Vec<ContactList>> {
        if let Some(cached) = self.cached_contact_lists().await {
            return Ok(cached);
        }
        let fresh = contacts::list_contact_lists(&self.client)
            .await
            .map_err(to_core_error)?;
        *self.contact_lists_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_contacts(&self, list_id: &str) -> CoreResult<Vec<Contact>> {
        contacts::get_contacts(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn search_contacts(&self, query: &str) -> CoreResult<Vec<Contact>> {
        // EWS supports server-side `Restriction` queries but their
        // shape varies across Exchange releases (CompanyName matching
        // doesn't compose with EmailAddresses matching in older
        // servers). Aperio's caches make the client-side grep cheap:
        // list books → for each book fetch its contacts → filter.
        // An empty / whitespace query short-circuits so a stray
        // keystroke doesn't trigger network traffic.
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let lists = self.list_contact_lists().await?;
        let mut out = Vec::new();
        for list in lists {
            // Tolerate per-list failures — a broken book shouldn't
            // mute the whole search.
            let Ok(rows) = self.get_contacts(&list.id).await else {
                continue;
            };
            for c in rows {
                if contacts::contact_matches(&c, &needle) {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    async fn create_contact(
        &self,
        list_id: &str,
        contact: NewContact,
    ) -> CoreResult<Contact> {
        contacts::create_contact(&self.client, list_id, contact)
            .await
            .map_err(to_core_error)
    }

    async fn update_contact(&self, contact: Contact) -> CoreResult<Contact> {
        contacts::update_contact(&self.client, &contact)
            .await
            .map_err(to_core_error)
    }

    async fn delete_contact(&self, contact_id: &str) -> CoreResult<()> {
        contacts::delete_contact(&self.client, contact_id)
            .await
            .map_err(to_core_error)
    }

    async fn rename_contact_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        contacts::rename_contact_list(&self.client, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        // Mirror the calendar / tasks pattern: drop the cache so the
        // next list_contact_lists picks up the new display name.
        *self.contact_lists_cache.lock().await = None;
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
