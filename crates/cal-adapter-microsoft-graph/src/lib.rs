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
    Adapter, AttendeeStatus, AuthToken, Calendar, CalendarFeature, Capability, ChangeSet, Contact,
    ContactList, ContactPhoto, ContactsFeature, ContainerColor, Credentials as CoreCredentials,
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
    contact_lists_cache: Mutex<Option<(Vec<ContactList>, chrono::DateTime<chrono::Utc>)>>,
    /// Per-`list_id` cache for `/me/contactFolders/{id}/contacts`
    /// + `/me/people`. Keyed by list_id so a write against one
    /// folder doesn't time out the others; a full clear on any
    /// mutation covers the Suggested People stream too (it's
    /// derived from cross-folder mailbox activity).
    contacts_cache: Mutex<HashMap<String, (Vec<Contact>, chrono::DateTime<chrono::Utc>)>>,
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
        self.listing_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
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

    /// Host-driven OAuth **authorize** phase (mobile): build the consent URL +
    /// PKCE verifier + CSRF state for a caller-supplied `redirect_uri` (e.g.
    /// `aperio://oauth-callback`). Pure — the host opens the URL in a native
    /// auth session, then calls [`Self::oauth_exchange`] with the returned code
    /// + this verifier/state. (Desktop instead uses the loopback
    /// [`Self::authenticate_interactive`].) `authority` selects the v2.0 tenant
    /// endpoint — pass the same value through to [`Self::oauth_exchange`].
    pub fn oauth_authorize(
        client_id: &str,
        authority: &str,
        redirect_uri: &str,
    ) -> GraphResult<auth::AuthorizeResponse> {
        auth::authorize(client_id, redirect_uri, &auth::authorize_url(authority))
    }

    /// Host-driven OAuth **exchange** phase (mobile): swap the authorization
    /// `code` for tokens. `redirect_uri` + `authority` must match the authorize
    /// call; the caller validates the CSRF state (returned vs. issued) before
    /// calling. No `client_secret` — Microsoft's v2.0 endpoint takes PKCE-only
    /// public-client exchanges.
    pub async fn oauth_exchange(
        client_id: &str,
        authority: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> GraphResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GraphError::Io(e.to_string()))?;
        auth::exchange_code(
            &http,
            &auth::token_url(authority),
            client_id,
            code,
            verifier,
            redirect_uri,
        )
        .await
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

    /// Bootstrap delta as a `full_resync` ChangeSet: every live event in
    /// `range` plus the initial `@odata.deltaLink`. Used on the no-token
    /// and 410-expired recovery paths.
    async fn full_events_changeset(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> CoreResult<ChangeSet<Event>> {
        let delta = api::initial_events_delta(&self.state, calendar_id, range.start, range.end)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes: delta.changes,
            deletions: Vec::new(),
            new_token: delta.new_token,
            full_resync: true,
            complete: false,
            unfetched: Vec::new(),
        })
    }

    /// Bootstrap To Do delta as a `full_resync` ChangeSet: every task in
    /// the list plus the initial `@odata.deltaLink`. Used on the no-token
    /// and 410-expired recovery paths.
    async fn full_tasks_changeset(&self, list_id: &str) -> CoreResult<ChangeSet<Task>> {
        let delta = api::initial_tasks_delta(&self.state, list_id)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes: delta.changes,
            deletions: Vec::new(),
            new_token: delta.new_token,
            full_resync: true,
            complete: false,
            unfetched: Vec::new(),
        })
    }

    /// Bootstrap a contact-folder delta as a `full_resync` ChangeSet:
    /// every contact in the folder plus the initial `@odata.deltaLink`.
    /// Used on the no-token and 410-expired recovery paths.
    async fn full_contacts_changeset(&self, folder_id: &str) -> CoreResult<ChangeSet<Contact>> {
        let delta = contacts::initial_contacts_delta(&self.state, folder_id)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes: delta.changes,
            deletions: Vec::new(),
            new_token: delta.new_token,
            full_resync: true,
            complete: false,
            unfetched: Vec::new(),
        })
    }
}

#[async_trait]
impl Adapter for MicrosoftGraphAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
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
        *self.calendars_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> CoreResult<Vec<Event>> {
        api::get_events(&self.state, calendar_id, range.start, range.end)
            .await
            .map_err(to_core_error)
    }

    /// Host-driven incremental read (CACHE-8) via Graph's
    /// `calendarView/delta`.
    ///
    /// No prior token → a full window delta that also yields the initial
    /// `@odata.deltaLink` (the host stores it and replaces wholesale).
    /// With a token (a stored delta link) → an incremental round: changed
    /// occurrences in `changes` (range-filtered), removed/cancelled ids
    /// in `deletions`, and the refreshed delta link. A `410 Gone` means
    /// Graph expired the link — we transparently re-bootstrap a full sync.
    async fn get_events_delta(
        &self,
        calendar_id: &str,
        range: DateRange,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Event>> {
        let Some(delta_link) = since_token else {
            return self.full_events_changeset(calendar_id, range).await;
        };
        match api::follow_events_delta(&self.state, delta_link, calendar_id, range.start, range.end)
            .await
        {
            Ok(delta) => Ok(ChangeSet {
                changes: delta.changes,
                deletions: delta.deletions,
                new_token: delta.new_token,
                full_resync: false,
                complete: false,
                unfetched: Vec::new(),
            }),
            // Delta link expired / invalidated by Graph — re-bootstrap.
            Err(GraphError::Http { status: 410, .. }) => {
                tracing::warn!(
                    target: "cal_adapter_microsoft_graph",
                    calendar = %calendar_id,
                    "Graph delta link expired (410); doing a full re-sync",
                );
                self.full_events_changeset(calendar_id, range).await
            }
            Err(err) => Err(to_core_error(err)),
        }
    }

    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> CoreResult<Event> {
        api::create_event(&self.state, calendar_id, event)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        api::update_event(&self.state, &event)
            .await
            .map_err(to_core_error)
    }

    async fn delete_event(&self, event_id: &str, send_cancellations: bool) -> CoreResult<()> {
        // Graph's event-id is mailbox-wide unique — no calendar
        // walk required, unlike Google.
        if send_cancellations {
            // Organizer cancellation: notify attendees first (Graph marks the
            // event cancelled), then remove it. Graph's `/cancel` is
            // ORGANIZER-ONLY, so a non-organizer gets 403 and a non-meeting /
            // non-cancellable item gets 400. Match the tolerance of the EWS
            // `DeleteItem` disposition and Google's `sendUpdates` query param:
            // on ONLY those "you can't cancel this" rejections, fall back to a
            // plain delete — there's nothing to notify, and removing your own
            // copy must still work (regression guard for the list/chip-menu
            // delete surfaces that pass send_cancellations = attendees>0 without
            // an organizer check). Every other error — 401 auth, 404 gone, 409
            // conflict, 429 throttling, any 5xx, network — PROPAGATES, so a
            // transient failure doesn't silently drop the cancellation and
            // delete anyway.
            match api::cancel_event(&self.state, event_id).await {
                Ok(()) => {}
                Err(GraphError::Http { status, .. }) if status == 400 || status == 403 => {
                    tracing::debug!(
                        status,
                        event_id,
                        "graph /cancel rejected (non-organizer / not cancellable); plain delete",
                    );
                }
                Err(err) => return Err(to_core_error(err)),
            }
        }
        api::delete_event(&self.state, event_id)
            .await
            .map_err(to_core_error)
    }

    async fn get_free_busy(&self, emails: &[&str], range: DateRange) -> CoreResult<Vec<FreeBusy>> {
        if emails.is_empty() {
            return Ok(Vec::new());
        }
        api::query_free_busy(&self.state, emails, range)
            .await
            .map_err(to_core_error)
    }

    async fn current_user_email(&self) -> CoreResult<Option<String>> {
        // Best-effort: a failed /me (revoked token) degrades to None so
        // the RSVP UI hides rather than erroring.
        Ok(api::current_user_email(&self.state).await.unwrap_or(None))
    }

    async fn respond_to_event(
        &self,
        event_id: &str,
        status: AttendeeStatus,
        send_response: bool,
    ) -> CoreResult<()> {
        api::respond_to_event(&self.state, event_id, status, send_response)
            .await
            .map_err(to_core_error)
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        None
    }

    async fn rename_calendar(&self, calendar_id: &str, new_name: &str) -> CoreResult<()> {
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
        _send_cancellations: bool,
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
        *self.task_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        api::get_tasks(&self.state, list_id)
            .await
            .map_err(to_core_error)
    }

    /// Host-driven incremental task read (CACHE-8) via Microsoft To Do's
    /// `tasks/delta`. Same delta-link contract as `get_events_delta`, but
    /// tasks aren't windowed so there's no range. Removed tasks come back
    /// as `@removed` tombstones, emitted as their full `{list}|{task}`
    /// cal-core id. A `410 Gone` re-bootstraps a full sync.
    async fn get_tasks_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Task>> {
        let Some(delta_link) = since_token else {
            return self.full_tasks_changeset(list_id).await;
        };
        match api::follow_tasks_delta(&self.state, delta_link, list_id).await {
            Ok(delta) => Ok(ChangeSet {
                changes: delta.changes,
                deletions: delta.deletions,
                new_token: delta.new_token,
                full_resync: false,
                complete: false,
                unfetched: Vec::new(),
            }),
            Err(GraphError::Http { status: 410, .. }) => {
                tracing::warn!(
                    target: "cal_adapter_microsoft_graph",
                    list = %list_id,
                    "Graph To Do delta link expired (410); doing a full re-sync",
                );
                self.full_tasks_changeset(list_id).await
            }
            Err(err) => Err(to_core_error(err)),
        }
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> CoreResult<Task> {
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

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        api::rename_task_list(&self.state, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }

    async fn create_task_list(&self, name: &str, parent_id: Option<&str>) -> CoreResult<TaskList> {
        let created = api::create_task_list(&self.state, name, parent_id)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(created)
    }

    async fn delete_task_list(&self, list_id: &str) -> CoreResult<()> {
        api::delete_task_list(&self.state, list_id)
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
        *self.contact_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
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

    /// Host-driven incremental contact read (CACHE-8) via Graph's
    /// `contactFolders/{id}/contacts/delta`. Same delta-link contract as
    /// the other surfaces. The synthetic "Suggested People" list is backed
    /// by `/me/people`, which has no delta endpoint — it returns
    /// `Unsupported` so the host falls back to a full read. Removed
    /// contacts come back as `@removed` tombstones; a contact's id is
    /// already its native resource id. A `410 Gone` re-bootstraps.
    async fn get_contacts_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Contact>> {
        if list_id == contacts::GRAPH_SUGGESTED_PEOPLE_LIST_ID {
            return Err(CoreError::Unsupported(
                "Suggested People (/me/people) has no delta sync".into(),
            ));
        }
        let Some(delta_link) = since_token else {
            return self.full_contacts_changeset(list_id).await;
        };
        match contacts::follow_contacts_delta(&self.state, delta_link, list_id).await {
            Ok(delta) => Ok(ChangeSet {
                changes: delta.changes,
                deletions: delta.deletions,
                new_token: delta.new_token,
                full_resync: false,
                complete: false,
                unfetched: Vec::new(),
            }),
            Err(GraphError::Http { status: 410, .. }) => {
                tracing::warn!(
                    target: "cal_adapter_microsoft_graph",
                    list = %list_id,
                    "Graph contacts delta link expired (410); doing a full re-sync",
                );
                self.full_contacts_changeset(list_id).await
            }
            Err(err) => Err(to_core_error(err)),
        }
    }

    async fn search_contacts(&self, query: &str) -> CoreResult<Vec<Contact>> {
        contacts::search_contacts(&self.state, query)
            .await
            .map_err(to_core_error)
    }

    async fn create_contact(&self, list_id: &str, contact: NewContact) -> CoreResult<Contact> {
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

    async fn rename_contact_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
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

    async fn get_contact_photo(&self, contact_id: &str) -> CoreResult<Option<ContactPhoto>> {
        contacts::get_contact_photo(&self.state, contact_id)
            .await
            .map_err(to_core_error)
    }

    async fn set_contact_photo(&self, contact_id: &str, photo: ContactPhoto) -> CoreResult<()> {
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
        AuthTimeout => CoreError::Authentication("consent screen timed out".into()),
        Csrf => CoreError::Protocol("CSRF state mismatch on OAuth callback".into()),
        Io(m) => CoreError::Internal(m),
        Config(m) => CoreError::InvalidInput(m),
    }
}

#[cfg(test)]
mod delta_tests {
    //! `get_events_delta` against a mocked Graph `calendarView/delta`.
    use super::*;
    use chrono::TimeZone;
    use mockito::{Matcher, Server};

    fn adapter_for(server: &Server) -> MicrosoftGraphAdapter {
        let mut adapter = MicrosoftGraphAdapter::new(
            "client".into(),
            "common".into(),
            TokenSet {
                access_token: "tok".into(),
                refresh_token: Some("refresh".into()),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                scope: None,
            },
        );
        adapter.state.api_base = server.url();
        adapter.state.token_url = format!("{}/token", server.url());
        adapter
    }

    fn range() -> DateRange {
        DateRange::new(
            chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
        )
    }

    fn one_event_with_delta_link(link: &str) -> String {
        r##"{
          "value": [
            {"id":"e1","subject":"One","isAllDay":false,"isReminderOn":false,
             "start":{"dateTime":"2026-05-10T08:00:00","timeZone":"UTC"},
             "end":{"dateTime":"2026-05-10T09:00:00","timeZone":"UTC"}}
          ],
          "@odata.deltaLink": "DELTA_LINK"
        }"##
        .replace("DELTA_LINK", link)
    }

    #[tokio::test]
    async fn no_token_does_full_resync_and_returns_delta_link() {
        let mut server = Server::new_async().await;
        let link = format!(
            "{}/me/calendars/cal-1/calendarView/delta?$deltatoken=DT1",
            server.url()
        );
        server
            .mock(
                "GET",
                Matcher::Regex(
                    r"^/me/calendars/cal-1/calendarView/delta\?startDateTime=".to_string(),
                ),
            )
            .with_status(200)
            .with_body(one_event_with_delta_link(&link))
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_events_delta("cal-1", range(), None)
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert!(cs.deletions.is_empty());
        assert_eq!(cs.new_token.as_deref(), Some(link.as_str()));
    }

    #[tokio::test]
    async fn cancel_non_organizer_falls_back_to_plain_delete() {
        // send_cancellations=true from a NON-organizer surface: Graph's
        // organizer-only /cancel returns 4xx, and delete_event must still remove
        // the event via a plain DELETE rather than aborting (regression guard).
        let mut server = Server::new_async().await;
        let cancel = server
            .mock("POST", "/me/events/ev-x/cancel")
            .with_status(403)
            .with_body(
                r#"{"error":{"code":"ErrorCannotCancelMeetingForNonOrganizer","message":"no"}}"#,
            )
            .create_async()
            .await;
        let del = server
            .mock("DELETE", "/me/events/ev-x")
            .with_status(204)
            .create_async()
            .await;
        adapter_for(&server)
            .delete_event("ev-x", true)
            .await
            .unwrap();
        cancel.assert_async().await;
        del.assert_async().await;
    }

    #[tokio::test]
    async fn cancel_throttled_propagates_and_does_not_delete() {
        // A TRANSIENT /cancel failure (429/5xx) must NOT fall through to a plain
        // delete — that would drop the cancellation and remove the meeting
        // anyway. It propagates so the caller can retry.
        let mut server = Server::new_async().await;
        server
            .mock("POST", "/me/events/ev-x/cancel")
            .with_status(429)
            .with_body(r#"{"error":{"code":"TooManyRequests","message":"slow down"}}"#)
            .create_async()
            .await;
        // No DELETE mock: if delete_event wrongly falls back, the request 501s
        // (mockito default) and the test still fails on the unwrap_err shape —
        // but the intent is that DELETE is never issued.
        let err = adapter_for(&server)
            .delete_event("ev-x", true)
            .await
            .unwrap_err();
        // 429 → CoreError::Protocol (to_core_error's catch-all), NOT swallowed.
        assert!(matches!(err, cal_core::Error::Protocol(_)));
    }

    #[tokio::test]
    async fn token_does_incremental_with_changes_and_removals() {
        let mut server = Server::new_async().await;
        let prev = format!(
            "{}/me/calendars/cal-1/calendarView/delta?$deltatoken=DT1",
            server.url()
        );
        let next = format!(
            "{}/me/calendars/cal-1/calendarView/delta?$deltatoken=DT2",
            server.url()
        );
        let body = r##"{
          "value": [
            {"id":"e1","subject":"Updated","isAllDay":false,"isReminderOn":false,
             "start":{"dateTime":"2026-05-10T08:00:00","timeZone":"UTC"},
             "end":{"dateTime":"2026-05-10T09:00:00","timeZone":"UTC"}},
            {"id":"e2","@removed":{"reason":"deleted"}}
          ],
          "@odata.deltaLink": "NEXT_LINK"
        }"##
        .replace("NEXT_LINK", &next);
        server
            .mock("GET", Matcher::Regex(r"deltatoken=DT1".to_string()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_events_delta("cal-1", range(), Some(&prev))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(
            cs.changes.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["e1"]
        );
        assert_eq!(cs.deletions, vec!["e2".to_string()]);
        assert_eq!(cs.new_token.as_deref(), Some(next.as_str()));
    }

    #[tokio::test]
    async fn expired_link_410_falls_back_to_full_resync() {
        let mut server = Server::new_async().await;
        let stale = format!(
            "{}/me/calendars/cal-1/calendarView/delta?$deltatoken=STALE",
            server.url()
        );
        let fresh = format!(
            "{}/me/calendars/cal-1/calendarView/delta?$deltatoken=DT3",
            server.url()
        );
        // Incremental call with the stale link → 410.
        server
            .mock("GET", Matcher::Regex(r"deltatoken=STALE".to_string()))
            .with_status(410)
            .with_body(r#"{"error":{"code":"syncStateNotFound"}}"#)
            .create_async()
            .await;
        // Recovery: a fresh full window delta with a new link.
        server
            .mock(
                "GET",
                Matcher::Regex(
                    r"^/me/calendars/cal-1/calendarView/delta\?startDateTime=".to_string(),
                ),
            )
            .with_status(200)
            .with_body(one_event_with_delta_link(&fresh))
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_events_delta("cal-1", range(), Some(&stale))
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.new_token.as_deref(), Some(fresh.as_str()));
    }

    fn one_task_with_delta_link(link: &str) -> String {
        r##"{
          "value": [
            {"id":"T1","title":"Buy milk","importance":"normal","status":"notStarted"}
          ],
          "@odata.deltaLink": "DELTA_LINK"
        }"##
        .replace("DELTA_LINK", link)
    }

    #[tokio::test]
    async fn no_token_does_full_tasks_resync() {
        let mut server = Server::new_async().await;
        let link = format!(
            "{}/me/todo/lists/LIST/tasks/delta?$deltatoken=DT1",
            server.url()
        );
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/me/todo/lists/LIST/tasks/delta".to_string()),
            )
            .with_status(200)
            .with_body(one_task_with_delta_link(&link))
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_tasks_delta("LIST", None)
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(
            cs.changes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["LIST|T1"]
        );
        assert!(cs.deletions.is_empty());
        assert_eq!(cs.new_token.as_deref(), Some(link.as_str()));
    }

    #[tokio::test]
    async fn token_does_incremental_tasks_with_changes_and_removals() {
        let mut server = Server::new_async().await;
        let prev = format!(
            "{}/me/todo/lists/LIST/tasks/delta?$deltatoken=DT1",
            server.url()
        );
        let next = format!(
            "{}/me/todo/lists/LIST/tasks/delta?$deltatoken=DT2",
            server.url()
        );
        let body = r##"{
          "value": [
            {"id":"T1","title":"Buy oat milk","importance":"normal","status":"notStarted"},
            {"id":"T2","@removed":{"reason":"deleted"}}
          ],
          "@odata.deltaLink": "NEXT_LINK"
        }"##
        .replace("NEXT_LINK", &next);
        server
            .mock("GET", Matcher::Regex(r"deltatoken=DT1".to_string()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_tasks_delta("LIST", Some(&prev))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(
            cs.changes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["LIST|T1"]
        );
        assert_eq!(cs.deletions, vec!["LIST|T2".to_string()]);
        assert_eq!(cs.new_token.as_deref(), Some(next.as_str()));
    }

    #[tokio::test]
    async fn expired_tasks_link_410_falls_back_to_full_resync() {
        let mut server = Server::new_async().await;
        let stale = format!(
            "{}/me/todo/lists/LIST/tasks/delta?$deltatoken=STALE",
            server.url()
        );
        let fresh = format!(
            "{}/me/todo/lists/LIST/tasks/delta?$deltatoken=DT3",
            server.url()
        );
        // Incremental call with the stale link → 410.
        server
            .mock("GET", Matcher::Regex(r"deltatoken=STALE".to_string()))
            .with_status(410)
            .with_body(r#"{"error":{"code":"syncStateNotFound"}}"#)
            .create_async()
            .await;
        // Recovery: a fresh full tasks delta (no $deltatoken on the URL).
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/me/todo/lists/LIST/tasks/delta$".to_string()),
            )
            .with_status(200)
            .with_body(one_task_with_delta_link(&fresh))
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_tasks_delta("LIST", Some(&stale))
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.new_token.as_deref(), Some(fresh.as_str()));
    }

    fn one_contact_with_delta_link(link: &str) -> String {
        r##"{
          "value": [ {"id":"c1","displayName":"Alice"} ],
          "@odata.deltaLink": "DELTA_LINK"
        }"##
        .replace("DELTA_LINK", link)
    }

    #[tokio::test]
    async fn no_token_does_full_contacts_resync() {
        let mut server = Server::new_async().await;
        let link = format!(
            "{}/me/contactFolders/folder-1/contacts/delta?$deltatoken=DC1",
            server.url()
        );
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/me/contactFolders/folder-1/contacts/delta".to_string()),
            )
            .with_status(200)
            .with_body(one_contact_with_delta_link(&link))
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_contacts_delta("folder-1", None)
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(
            cs.changes.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["c1"]
        );
        assert!(cs.deletions.is_empty());
        assert_eq!(cs.new_token.as_deref(), Some(link.as_str()));
    }

    #[tokio::test]
    async fn token_does_incremental_contacts_with_changes_and_removals() {
        let mut server = Server::new_async().await;
        let prev = format!(
            "{}/me/contactFolders/folder-1/contacts/delta?$deltatoken=DC1",
            server.url()
        );
        let next = format!(
            "{}/me/contactFolders/folder-1/contacts/delta?$deltatoken=DC2",
            server.url()
        );
        let body = r##"{
          "value": [
            {"id":"c1","displayName":"Alice Cooper"},
            {"id":"c2","@removed":{"reason":"deleted"}}
          ],
          "@odata.deltaLink": "NEXT_LINK"
        }"##
        .replace("NEXT_LINK", &next);
        server
            .mock("GET", Matcher::Regex(r"deltatoken=DC1".to_string()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_contacts_delta("folder-1", Some(&prev))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(
            cs.changes.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["c1"]
        );
        // A contact's id is already its native id — emit it bare.
        assert_eq!(cs.deletions, vec!["c2".to_string()]);
        assert_eq!(cs.new_token.as_deref(), Some(next.as_str()));
    }

    #[tokio::test]
    async fn expired_contacts_link_410_falls_back_to_full_resync() {
        let mut server = Server::new_async().await;
        let stale = format!(
            "{}/me/contactFolders/folder-1/contacts/delta?$deltatoken=STALE",
            server.url()
        );
        let fresh = format!(
            "{}/me/contactFolders/folder-1/contacts/delta?$deltatoken=DC3",
            server.url()
        );
        // Incremental call with the stale link → 410.
        server
            .mock("GET", Matcher::Regex(r"deltatoken=STALE".to_string()))
            .with_status(410)
            .with_body(r#"{"error":{"code":"syncStateNotFound"}}"#)
            .create_async()
            .await;
        // Recovery: a fresh full folder delta (carries $select, no token).
        server
            .mock(
                "GET",
                Matcher::Regex(r"contacts/delta.*select=".to_string()),
            )
            .with_status(200)
            .with_body(one_contact_with_delta_link(&fresh))
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_contacts_delta("folder-1", Some(&stale))
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.new_token.as_deref(), Some(fresh.as_str()));
    }

    #[tokio::test]
    async fn suggested_people_list_has_no_delta() {
        let server = Server::new_async().await;
        // The synthetic /me/people-backed list can't delta — it must
        // surface Unsupported so the host falls back to a full read.
        let err = adapter_for(&server)
            .get_contacts_delta(crate::contacts::GRAPH_SUGGESTED_PEOPLE_LIST_ID, None)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Unsupported(_)));
    }
}
