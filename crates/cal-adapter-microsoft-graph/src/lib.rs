//! Microsoft Graph adapter (DESIGN.md §6.2, Phase 6e).
//!
//! OAuth 2.0 PKCE against the Microsoft Identity Platform v2.0 endpoint
//! + the Graph Calendar API. Same architectural shape as the Google
//! adapter; the differences (no client_secret, structured recurrence,
//! `calendarView` for range expansion, `nextLink` pagination) live in
//! the `auth`, `api` and `mapping` modules.
//!
//! Setup requirement: the user registers an app of type "Public
//! client" in Azure Portal → Entra ID → App registrations, sets the
//! redirect URI to `http://localhost` (loopback IP pattern), and
//! pastes the client id into Aperio. No client_secret is needed —
//! Microsoft honours the PKCE-public-client model the spec
//! describes.

pub mod api;
pub mod auth;
pub mod contacts;
pub mod error;
pub mod mapping;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, Contact, ContactList,
    ContactPhoto, ContactsFeature, ContainerColor, Credentials as CoreCredentials,
    DateRange, Error as CoreError, Event, FreeBusy, NewContact, NewEvent, NewTask,
    Result as CoreResult, Task, TaskList, TasksFeature,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub use auth::{TokenSet, DEFAULT_AUTHORITY};
pub use error::{GraphError, GraphResult};

use crate::api::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphAccountConfig {
    pub client_id: String,
    /// `common` | `consumers` | `organizations` | tenant GUID. Most
    /// users want `common` so both personal Microsoft accounts and
    /// work/school accounts can authenticate; admins who want to
    /// pin a tenant can override.
    #[serde(default = "default_authority")]
    pub authority: String,
    #[serde(default)]
    pub account_label: Option<String>,
}

fn default_authority() -> String {
    DEFAULT_AUTHORITY.to_string()
}

#[derive(Debug)]
pub struct MicrosoftGraphAdapter {
    state: ApiState,
    capabilities: Vec<Capability>,
    calendars_cache: Mutex<Option<(Vec<Calendar>, chrono::DateTime<chrono::Utc>)>>,
    /// Same 5-minute TTL cache pattern as `calendars_cache`, applied
    /// to `/me/todo/lists` so the sidebar's task-list column doesn't
    /// re-fetch on every navigation.
    task_lists_cache: Mutex<Option<(Vec<TaskList>, chrono::DateTime<chrono::Utc>)>>,
    /// Listing cache for `/me/contactFolders` + the Suggested
    /// People sentinel. Single entry — `list_contact_lists`
    /// returns the full set in one call.
    contact_lists_cache:
        Mutex<Option<(Vec<ContactList>, chrono::DateTime<chrono::Utc>)>>,
    /// Per-`list_id` cache for `/me/contactFolders/{id}/contacts`
    /// + `/me/people`. Keyed by list_id so a write against one
    /// folder doesn't time out the others; a full clear on any
    /// mutation covers the Suggested People stream too (it's
    /// derived from cross-folder mailbox activity).
    contacts_cache:
        Mutex<HashMap<String, (Vec<Contact>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl MicrosoftGraphAdapter {
    pub fn new(client_id: String, authority: String, tokens: TokenSet) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            state: ApiState::new(tokens, client_id, auth::token_url(&authority), http),
            // Phase 6e.1 + 6e.2 added Calendar + Tasks; Phase 10i
            // adds Contacts. Declaring all three lets the registry
            // wire the single adapter Arc under every feature
            // surface, so the OAuth token state + listing caches
            // stay coherent across reads.
            capabilities: vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts,
            ],
            calendars_cache: Mutex::new(None),
            task_lists_cache: Mutex::new(None),
            contact_lists_cache: Mutex::new(None),
            contacts_cache: Mutex::new(HashMap::new()),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl = chrono::Duration::from_std(ttl)
            .unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    pub async fn authenticate_interactive(
        client_id: &str,
        authority: &str,
    ) -> GraphResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GraphError::Io(e.to_string()))?;
        auth::run_default(client_id, authority, &http).await
    }

    pub async fn current_tokens(&self) -> TokenSet {
        self.state.tokens.lock().await.clone()
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

    async fn cached_contacts(&self, list_id: &str) -> Option<Vec<Contact>> {
        let guard = self.contacts_cache.lock().await;
        let (items, ts) = guard.get(list_id)?;
        let age = chrono::Utc::now().signed_duration_since(*ts);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(items.clone())
        } else {
            None
        }
    }
}

#[async_trait]
impl Adapter for MicrosoftGraphAdapter {
    async fn authenticate(
        &self,
        _credentials: CoreCredentials,
    ) -> CoreResult<AuthToken> {
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for MicrosoftGraphAdapter {
    async fn list_calendars(&self) -> CoreResult<Vec<Calendar>> {
        if let Some(cached) = self.cached_calendars().await {
            return Ok(cached);
        }
        let fresh = api::list_calendars(&self.state)
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
        api::get_events(&self.state, calendar_id, range.start, range.end)
            .await
            .map_err(to_core_error)
    }

    async fn create_event(
        &self,
        calendar_id: &str,
        event: NewEvent,
    ) -> CoreResult<Event> {
        api::create_event(&self.state, calendar_id, event)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        api::update_event(&self.state, &event)
            .await
            .map_err(to_core_error)
    }

    async fn delete_event(&self, event_id: &str) -> CoreResult<()> {
        // Graph's event-id is mailbox-wide unique — no calendar
        // walk required, unlike Google.
        api::delete_event(&self.state, event_id)
            .await
            .map_err(to_core_error)
    }

    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> CoreResult<Vec<FreeBusy>> {
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        None
    }

    async fn rename_calendar(
        &self,
        calendar_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        api::rename_calendar(&self.state, calendar_id, new_name)
            .await
            .map_err(to_core_error)?;
        *self.calendars_cache.lock().await = None;
        Ok(())
    }

    async fn add_event_exdate(
        &self,
        _event_id: &str,
        _occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        // Graph exposes per-occurrence cancellation through its
        // "event series exception" endpoint, which works against
        // an instance id of an expanded series. Since we read via
        // `/calendarView` (server-side expansion) the instance ids
        // are stable per occurrence — but the API also accepts a
        // PATCH on the master event with a structured cancellation
        // shape that's distinct from RRULE EXDATE. Keep it
        // `Unsupported` until the per-occurrence cancellation
        // endpoint is wired up properly in a follow-up; the
        // frontend's "delete only this occurrence" still works for
        // CalDAV / iCal / Google / local.
        Err(CoreError::Unsupported(
            "Microsoft Graph per-occurrence delete lands in a follow-up commit".into(),
        ))
    }
}

#[async_trait]
impl TasksFeature for MicrosoftGraphAdapter {
    async fn list_task_lists(&self) -> CoreResult<Vec<TaskList>> {
        if let Some(cached) = self.cached_task_lists().await {
            return Ok(cached);
        }
        let fresh = api::list_task_lists(&self.state)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        api::get_tasks(&self.state, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(
        &self,
        list_id: &str,
        task: NewTask,
    ) -> CoreResult<Task> {
        api::create_task(&self.state, list_id, task)
            .await
            .map_err(to_core_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        api::update_task(&self.state, &task)
            .await
            .map_err(to_core_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        api::delete_task(&self.state, task_id)
            .await
            .map_err(to_core_error)
    }

    async fn rename_task_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        api::rename_task_list(&self.state, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }
}

#[async_trait]
impl ContactsFeature for MicrosoftGraphAdapter {
    async fn list_contact_lists(&self) -> CoreResult<Vec<ContactList>> {
        if let Some(cached) = self.cached_contact_lists().await {
            return Ok(cached);
        }
        let fresh = contacts::list_contact_lists(&self.state)
            .await
            .map_err(to_core_error)?;
        *self.contact_lists_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_contacts(&self, list_id: &str) -> CoreResult<Vec<Contact>> {
        if let Some(cached) = self.cached_contacts(list_id).await {
            return Ok(cached);
        }
        let fresh = contacts::get_contacts(&self.state, list_id)
            .await
            .map_err(to_core_error)?;
        self.contacts_cache
            .lock()
            .await
            .insert(list_id.to_string(), (fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn search_contacts(&self, query: &str) -> CoreResult<Vec<Contact>> {
        contacts::search_contacts(&self.state, query)
            .await
            .map_err(to_core_error)
    }

    async fn create_contact(
        &self,
        list_id: &str,
        contact: NewContact,
    ) -> CoreResult<Contact> {
        if is_read_only_graph_list(list_id) {
            return Err(CoreError::Unsupported(format!(
                "the Graph list '{list_id}' is read-only and does not accept new contacts"
            )));
        }
        let result = contacts::create_contact(&self.state, list_id, contact)
            .await
            .map_err(to_core_error)?;
        // A new row across any folder might also surface in the
        // Suggested People stream the next time Graph runs its
        // relevance scoring — clear every cache slot.
        self.contacts_cache.lock().await.clear();
        Ok(result)
    }

    async fn update_contact(&self, contact: Contact) -> CoreResult<Contact> {
        if is_read_only_graph_list(&contact.list_id) {
            return Err(CoreError::Unsupported(format!(
                "contacts in '{}' cannot be edited from Aperio",
                contact.list_id,
            )));
        }
        let result = contacts::update_contact(&self.state, contact)
            .await
            .map_err(to_core_error)?;
        self.contacts_cache.lock().await.clear();
        Ok(result)
    }

    async fn delete_contact(&self, contact_id: &str) -> CoreResult<()> {
        contacts::delete_contact(&self.state, contact_id)
            .await
            .map_err(to_core_error)?;
        self.contacts_cache.lock().await.clear();
        Ok(())
    }

    async fn rename_contact_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        if is_read_only_graph_list(list_id) {
            return Err(CoreError::Unsupported(
                "the Suggested People list is synthetic and cannot be renamed".into(),
            ));
        }
        let id_enc = urlencoding(list_id);
        let path = format!("/me/contactFolders/{id_enc}");
        let body = serde_json::json!({ "displayName": new_name });
        let _: serde_json::Value = self
            .state
            .patch_json(&path, &body)
            .await
            .map_err(to_core_error)?;
        // Drop the cached folder listing — the cached display name
        // is stale after the rename lands.
        *self.contact_lists_cache.lock().await = None;
        Ok(())
    }

    async fn get_contact_photo(
        &self,
        contact_id: &str,
    ) -> CoreResult<Option<ContactPhoto>> {
        contacts::get_contact_photo(&self.state, contact_id)
            .await
            .map_err(to_core_error)
    }

    async fn set_contact_photo(
        &self,
        contact_id: &str,
        photo: ContactPhoto,
    ) -> CoreResult<()> {
        contacts::set_contact_photo(&self.state, contact_id, photo)
            .await
            .map_err(to_core_error)?;
        self.contacts_cache.lock().await.clear();
        Ok(())
    }

    async fn delete_contact_photo(&self, contact_id: &str) -> CoreResult<()> {
        contacts::delete_contact_photo(&self.state, contact_id)
            .await
            .map_err(to_core_error)?;
        self.contacts_cache.lock().await.clear();
        Ok(())
    }

    async fn invalidate_contacts_cache(&self) -> CoreResult<()> {
        // Drop both the folder-listing snapshot and the per-list
        // contact arrays. The next `list_contact_lists` /
        // `get_contacts` hits Graph and re-warms.
        *self.contact_lists_cache.lock().await = None;
        self.contacts_cache.lock().await.clear();
        Ok(())
    }
}

/// True for the synthetic Suggested People sentinel; the Outlook
/// contactFolders themselves are all writable. The frontend's
/// read-only-aware dialog short-circuits this too, but the
/// backend guard is the authoritative gate.
fn is_read_only_graph_list(list_id: &str) -> bool {
    list_id == contacts::GRAPH_SUGGESTED_PEOPLE_LIST_ID
}

/// Same percent-encoder pattern other modules use. We need it
/// here for the `rename_contact_list` path encoding — every
/// other contact route is built inside `contacts::*`.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn to_core_error(err: GraphError) -> CoreError {
    use GraphError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("Graph HTTP {status}: {message}")),
        },
        Protocol(m) => CoreError::Protocol(m),
        AuthDenied(m) => CoreError::Authentication(format!("denied: {m}")),
        AuthTimeout => CoreError::Authentication(
            "consent screen timed out".into(),
        ),
        Csrf => CoreError::Protocol(
            "CSRF state mismatch on OAuth callback".into(),
        ),
        Io(m) => CoreError::Internal(m),
        Config(m) => CoreError::InvalidInput(m),
    }
}
