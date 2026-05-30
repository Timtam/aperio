//! Google Calendar adapter (DESIGN.md §6.2, Phase 6d).
//!
//! Implements OAuth 2.0 PKCE for the desktop flow plus the read half
//! of Google's Calendar API v3. Write paths (`create_event` /
//! `update_event` / `delete_event` / `rename_calendar`) return
//! `Unsupported` in 6d.1 and land in 6d.2.
//!
//! **Setup requirement.** The user provides their own OAuth client id
//! out of the Google Cloud Console (Desktop app type). Aperio's
//! production registration / verification is a release-phase concern
//! and not bundled here. See the AccountsDialog help text for the
//! step-by-step.

pub mod api;
pub mod auth;
pub mod contacts;
pub mod error;
pub mod mapping;
pub mod tasks;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, Contact, ContactList, ContactPhoto,
    ContactsFeature, ContainerColor, Credentials as CoreCredentials, DateRange, Error as CoreError,
    Event, FreeBusy, NewContact, NewEvent, NewTask, Result as CoreResult, Task, TaskList,
    TasksFeature,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub use auth::TokenSet;
pub use error::{GoogleError, GoogleResult};

use crate::api::ApiState;

/// Persisted-in-the-DB part of a Google account's configuration. The
/// access + refresh tokens live in the keychain via the `secrets`
/// module — `client_id` and `client_secret` sit here in the DB.
///
/// "Why is the client_secret in the DB and not the keychain?"
/// Google's own Desktop-app OAuth documentation concedes that the
/// client secret in this flow "is not treated as a secret" —
/// installed apps are expected to embed it in the source code. We
/// adopt the same view: it's an identifier of the user's own
/// Google Cloud project, not a credential that would let an
/// attacker who reads it impersonate the user. Putting it next to
/// the client_id keeps the data layout coherent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleAccountConfig {
    /// OAuth 2.0 client id from the user's Google Cloud Console
    /// (Desktop app type). Treated as public information.
    pub client_id: String,
    /// The matching client_secret from the same Cloud Console
    /// entry. See the type-level doc for the "not really a secret"
    /// discussion.
    pub client_secret: String,
    /// User-visible email / display name the consent screen showed.
    /// Optional — populated after the first authentication and
    /// helpful when the user has multiple Google accounts.
    #[serde(default)]
    pub account_label: Option<String>,
}

#[derive(Debug)]
pub struct GoogleAdapter {
    state: ApiState,
    capabilities: Vec<Capability>,
    /// Listing cache mirrors the CalDAV adapter — list_calendars is
    /// expensive (~300 ms over HTTPS to Google) and the sidebar
    /// calls it from several refresh paths.
    calendars_cache: Mutex<Option<(Vec<Calendar>, chrono::DateTime<chrono::Utc>)>>,
    /// Same shape as `calendars_cache` but for Google Tasks lists.
    /// The tasks API lives on a separate host — caching the listing
    /// keeps the sidebar snappy and avoids re-hitting Google on every
    /// `list_task_lists` call.
    task_lists_cache: Mutex<Option<(Vec<TaskList>, chrono::DateTime<chrono::Utc>)>>,
    /// Cache for the People API listing per list_id. Keyed by
    /// list_id so the three synthetic lists (personal + Other
    /// Contacts + Directory) each cache independently and a
    /// mutation against one doesn't invalidate the others'
    /// timing-out work. 5-minute TTL keeps a panel re-open
    /// instant; full clear on any mutation in case the mutation
    /// touches relationships across lists (rare but possible
    /// via group membership changes that affect personal +
    /// directory views).
    contacts_cache: Mutex<HashMap<String, (Vec<Contact>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl GoogleAdapter {
    /// Construct an adapter from a previously-obtained [`TokenSet`].
    /// Use [`Self::authenticate_interactive`] to run the OAuth dance
    /// first when creating a fresh account.
    pub fn new(client_id: String, client_secret: String, tokens: TokenSet) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            state: ApiState::new(tokens, client_id, client_secret, http),
            // Phase 6d.3 added Tasks; Phase 10h adds Contacts.
            // Declaring all three capabilities lets the registry
            // route every feature surface through this one
            // adapter instance so the shared OAuth token state +
            // listing caches stay coherent.
            capabilities: vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts,
            ],
            calendars_cache: Mutex::new(None),
            task_lists_cache: Mutex::new(None),
            contacts_cache: Mutex::new(HashMap::new()),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    /// Override the listing-cache freshness window. Production
    /// callers stick with the 5-minute default; tests pin it to
    /// zero to keep the network-fetch path reachable.
    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    /// Run the interactive OAuth dance: localhost listener, browser
    /// open, code exchange. Returns the resulting [`TokenSet`] so
    /// the caller can persist `refresh_token` to keychain.
    pub async fn authenticate_interactive(
        client_id: &str,
        client_secret: &str,
    ) -> GoogleResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GoogleError::Io(e.to_string()))?;
        auth::run_default(client_id, client_secret, &http).await
    }

    /// Expose the current in-memory tokens. Used after a refresh so
    /// the wrapping code (Tauri command, account-creation flow) can
    /// persist the updated access / refresh token back to keychain.
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
impl Adapter for GoogleAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
        // Google's auth happens before adapter construction (the
        // OAuth dance runs in `authenticate_interactive` and stores
        // tokens in keychain). Once the adapter exists, tokens are
        // already in hand and the trait method is a no-op stub —
        // refreshing happens lazily inside the API client on 401.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for GoogleAdapter {
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

    async fn delete_event(&self, event_id: &str) -> CoreResult<()> {
        // Aperio's command layer hands us the calendar_id alongside
        // the event_id when it can, but the legacy
        // `delete_event(event_id)` trait method doesn't carry it.
        // We walk every calendar in the listing cache and try the
        // delete against each — the first 2xx wins, the rest get
        // their 404s swallowed. Mirrors how the CalDAV adapter
        // copes with the same signature gap.
        let cals = self.list_calendars().await?;
        for cal in cals {
            if api::delete_event(&self.state, &cal.id, event_id)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "event '{event_id}' not found in any calendar"
        )))
    }

    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> CoreResult<Vec<FreeBusy>> {
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // Colour is read together with the listing; consumers should
        // use the value on the Calendar struct rather than asking
        // the adapter again.
        None
    }

    async fn rename_calendar(&self, calendar_id: &str, new_name: &str) -> CoreResult<()> {
        api::rename_calendar(&self.state, calendar_id, new_name)
            .await
            .map_err(to_core_error)?;
        // The cached listing still has the old summary — drop it so
        // the next list_calendars walks Google again and surfaces
        // the new name.
        *self.calendars_cache.lock().await = None;
        Ok(())
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        // Same calendar-id-walking pattern as delete_event — the
        // trait method doesn't carry the calendar id.
        let cals = self.list_calendars().await?;
        for cal in cals {
            if api::add_event_exdate(&self.state, &cal.id, event_id, occurrence)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "event '{event_id}' not found in any calendar"
        )))
    }
}

#[async_trait]
impl TasksFeature for GoogleAdapter {
    async fn list_task_lists(&self) -> CoreResult<Vec<TaskList>> {
        if let Some(cached) = self.cached_task_lists().await {
            return Ok(cached);
        }
        let fresh = tasks::list_task_lists(&self.state)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        tasks::get_tasks(&self.state, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> CoreResult<Task> {
        tasks::create_task(&self.state, list_id, task)
            .await
            .map_err(to_core_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        tasks::update_task(&self.state, &task)
            .await
            .map_err(to_core_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        // The trait signature drops the list_id, so we walk every
        // known list and let the first 2xx win. Mirrors the
        // `delete_event` fallback above — same trade-off (one extra
        // round-trip per non-matching list) and same justification:
        // the Aperio command layer already knows the list, but the
        // trait surface does not carry it.
        let lists = self.list_task_lists().await?;
        for list in lists {
            if tasks::delete_task(&self.state, &list.id, task_id)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "task '{task_id}' not found in any task list"
        )))
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        tasks::rename_task_list(&self.state, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        // Drop the cached listing — the cached title is stale.
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }

    async fn create_task_list(&self, name: &str, parent_id: Option<&str>) -> CoreResult<TaskList> {
        let created = tasks::create_task_list(&self.state, name, parent_id)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(created)
    }

    async fn delete_task_list(&self, list_id: &str) -> CoreResult<()> {
        tasks::delete_task_list(&self.state, list_id)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }
}

#[async_trait]
impl ContactsFeature for GoogleAdapter {
    async fn list_contact_lists(&self) -> CoreResult<Vec<ContactList>> {
        // Google exposes exactly one synthetic ContactList per
        // account — the user's address book. Static, doesn't
        // need a fetch; the registry will still call this on
        // every sidebar refresh and we want it to be cheap.
        Ok(contacts::list_contact_lists())
    }

    async fn get_contacts(&self, list_id: &str) -> CoreResult<Vec<Contact>> {
        // Unknown ids yield an empty Vec — the registry shouldn't
        // route foreign list ids here, but defending makes the
        // failure mode boring.
        if list_id != contacts::GOOGLE_CONTACT_LIST_ID
            && list_id != contacts::GOOGLE_OTHER_CONTACTS_LIST_ID
            && list_id != contacts::GOOGLE_DIRECTORY_LIST_ID
        {
            return Ok(Vec::new());
        }
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

    async fn create_contact(&self, list_id: &str, contact: NewContact) -> CoreResult<Contact> {
        if is_read_only_google_list(list_id) {
            return Err(CoreError::Unsupported(format!(
                "the Google list '{list_id}' is read-only and does not accept new contacts"
            )));
        }
        let result = contacts::create_contact(&self.state, contact)
            .await
            .map_err(to_core_error)?;
        // Invalidate the listing cache so the next `get_contacts`
        // shows the newly created row.
        self.contacts_cache.lock().await.clear();
        Ok(result)
    }

    async fn update_contact(&self, contact: Contact) -> CoreResult<Contact> {
        if is_read_only_google_list(&contact.list_id) {
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

    async fn rename_contact_list(&self, _list_id: &str, _new_name: &str) -> CoreResult<()> {
        // The synthetic "Google Contacts" list isn't a real
        // server-side container — there's nothing to PATCH. The
        // command layer falls back to a local override on
        // Unsupported.
        Err(CoreError::Unsupported(
            "the Google Contacts list cannot be renamed at the source".into(),
        ))
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
        // The synthetic ContactList listing itself is static
        // (built from sentinel ids in contacts::list_contact_lists),
        // so there's nothing to drop on the list side. The
        // per-list contacts HashMap is the only stateful cache.
        self.contacts_cache.lock().await.clear();
        Ok(())
    }
}

/// True for the two synthetic Google ContactLists that don't
/// accept writes: Other Contacts (auto-collected by Gmail) and
/// the Workspace Directory. The frontend's read-only-aware
/// dialog short-circuits these too, but the backend guard is the
/// authoritative gate.
fn is_read_only_google_list(list_id: &str) -> bool {
    list_id == contacts::GOOGLE_OTHER_CONTACTS_LIST_ID
        || list_id == contacts::GOOGLE_DIRECTORY_LIST_ID
}

fn to_core_error(err: GoogleError) -> CoreError {
    use GoogleError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("Google HTTP {status}: {message}")),
        },
        Protocol(m) => CoreError::Protocol(m),
        AuthDenied(m) => CoreError::Authentication(format!("denied: {m}")),
        AuthTimeout => CoreError::Authentication("consent screen timed out".into()),
        Csrf => CoreError::Protocol("CSRF state mismatch on OAuth callback".into()),
        Io(m) => CoreError::Internal(m),
        Config(m) => CoreError::InvalidInput(m),
    }
}
