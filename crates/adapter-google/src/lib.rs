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
/// Google Drive: the storage role of the same account.
pub mod drive;
pub mod error;
pub mod mapping;
pub mod tasks;

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

    /// Host-driven OAuth **authorize** phase (mobile): build the consent URL +
    /// PKCE verifier + CSRF state for a caller-supplied `redirect_uri` (e.g.
    /// `aperio://oauth-callback`). Pure — the host opens the URL in a native
    /// auth session, then calls [`Self::oauth_exchange`] with the returned code
    /// + this verifier/state. (Desktop instead uses the loopback
    /// [`Self::authenticate_interactive`].)
    pub fn oauth_authorize(
        client_id: &str,
        redirect_uri: &str,
    ) -> GoogleResult<auth::AuthorizeResponse> {
        auth::authorize(client_id, redirect_uri, auth::GOOGLE_AUTH_URL)
    }

    /// Host-driven OAuth **exchange** phase (mobile): swap the authorization
    /// `code` for tokens. `redirect_uri` must match the authorize call; the
    /// caller validates the CSRF state (returned vs. issued) before calling.
    pub async fn oauth_exchange(
        client_id: &str,
        client_secret: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> GoogleResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GoogleError::Io(e.to_string()))?;
        auth::exchange_code(
            &http,
            auth::GOOGLE_TOKEN_URL,
            client_id,
            client_secret,
            code,
            verifier,
            redirect_uri,
        )
        .await
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

    /// Full window sync wrapped as a `full_resync` ChangeSet: every live
    /// event in `range` plus the fresh `nextSyncToken`. Used on the
    /// bootstrap (no token) and the 410-token-expired recovery paths.
    async fn full_events_changeset(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> CoreResult<ChangeSet<Event>> {
        let (changes, new_token) =
            api::list_events_full(&self.state, calendar_id, range.start, range.end)
                .await
                .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes,
            deletions: Vec::new(),
            new_token,
            full_resync: true,
            complete: false,
            unfetched: Vec::new(),
        })
    }

    /// Full Other-Contacts sync wrapped as a `full_resync` ChangeSet: the
    /// complete set plus the fresh syncToken. Used on the no-token and
    /// 400-expired recovery paths.
    async fn other_contacts_full_changeset(&self) -> CoreResult<ChangeSet<Contact>> {
        let (changes, new_token) = contacts::other_contacts_full(&self.state)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes,
            deletions: Vec::new(),
            new_token,
            full_resync: true,
            complete: false,
            unfetched: Vec::new(),
        })
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

    /// Host-driven incremental read (CACHE-8) via Google's
    /// `events.list` sync tokens.
    ///
    /// No prior token → a full window sync that also returns the
    /// `nextSyncToken` (the host stores it and replaces wholesale).
    /// With a token → an incremental sync: changed/created events in
    /// `changes` (singles range-filtered, masters kept), cancelled rows
    /// in `deletions` (their ids are already native), and the refreshed
    /// token. A `410 Gone` means Google expired the token — we transparently
    /// fall back to a full resync so the next round starts clean.
    async fn get_events_delta(
        &self,
        calendar_id: &str,
        range: DateRange,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Event>> {
        let Some(token) = since_token else {
            return self.full_events_changeset(calendar_id, range).await;
        };
        match api::list_events_incremental(&self.state, calendar_id, token, range.start, range.end)
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
            // Token expired / invalidated by Google — drop it and resync.
            Err(GoogleError::Http { status: 410, .. }) => {
                tracing::warn!(
                    target: "adapter_google",
                    calendar = %calendar_id,
                    "Google sync token expired (410); doing a full re-sync",
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
        // Aperio's command layer hands us the calendar_id alongside the event_id
        // when it can, but the legacy `delete_event(event_id)` trait method doesn't
        // carry it. We walk every calendar and try the delete against each. A 404 =
        // "not on this calendar" (keep walking); a 410 = "already gone" anywhere =
        // success (idempotent). Any OTHER error (403/412/5xx) on a calendar is a
        // real failure — remember it and surface it after the walk instead of
        // masking it as the misleading "not found in any calendar".
        let cals = self.list_calendars().await?;
        let mut real_error: Option<GoogleError> = None;
        for cal in cals {
            match api::delete_event(&self.state, &cal.id, event_id, send_cancellations).await {
                Ok(()) => return Ok(()),
                Err(GoogleError::Http { status: 404, .. }) => continue,
                Err(GoogleError::Http { status: 410, .. }) => return Ok(()),
                Err(e) => real_error = Some(e),
            }
        }
        if let Some(e) = real_error {
            return Err(to_core_error(e));
        }
        Err(CoreError::NotFound(format!(
            "event '{event_id}' not found in any calendar"
        )))
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
        // Best-effort: a failure (revoked token, missing scope) degrades
        // to None so the RSVP UI hides rather than surfacing an error.
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
        send_cancellations: bool,
    ) -> CoreResult<()> {
        // Same calendar-id-walking pattern as delete_event — the trait method
        // doesn't carry the calendar id. api::add_event_exdate tells us whether the
        // master lives on THIS calendar: `MasterNotHere` → keep walking; a real
        // error (the DELETE failed on the owning calendar) → surface it instead of
        // masking every failure as the misleading "not found in any calendar".
        let cals = self.list_calendars().await?;
        for cal in cals {
            match api::add_event_exdate(
                &self.state,
                &cal.id,
                event_id,
                occurrence,
                send_cancellations,
            )
            .await
            {
                Ok(api::ExdateOutcome::Cancelled) => return Ok(()),
                Ok(api::ExdateOutcome::MasterNotHere) => continue,
                Err(e) => return Err(to_core_error(e)),
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

    /// Host-driven incremental contact read (CACHE-8) via the People API
    /// `syncToken`.
    ///
    /// Only the **Other Contacts** list (`otherContacts.list`) deltas
    /// here: it's pure people (so resourceName = native id, tombstones via
    /// `metadata.deleted`) and is the large auto-collected list where
    /// per-resource sync matters most. The personal list couples each
    /// contact-group's member list to the FULL people set (a delta can't
    /// recompute it), and the Directory is read-only org data — both
    /// return `Unsupported` so the host keeps doing a correct full read.
    ///
    /// No token (or a 400-expired token) → a full sync that also captures
    /// the initial syncToken; otherwise an incremental round.
    async fn get_contacts_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Contact>> {
        if list_id != contacts::GOOGLE_OTHER_CONTACTS_LIST_ID {
            return Err(CoreError::Unsupported(
                "only the Google Other Contacts list supports delta sync".into(),
            ));
        }
        let Some(token) = since_token else {
            return self.other_contacts_full_changeset().await;
        };
        match contacts::other_contacts_delta(&self.state, token).await {
            Ok(delta) => Ok(ChangeSet {
                changes: delta.changes,
                deletions: delta.deletions,
                new_token: delta.new_token,
                full_resync: false,
                complete: false,
                unfetched: Vec::new(),
            }),
            // The People API expires sync tokens with a 400 — re-sync.
            Err(GoogleError::Http { status: 400, .. }) => {
                tracing::warn!(
                    target: "adapter_google::contacts",
                    "Other Contacts sync token expired (400); doing a full re-sync",
                );
                self.other_contacts_full_changeset().await
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

#[cfg(test)]
mod delta_tests {
    //! `get_events_delta` against a mocked Google `events.list`.
    use super::*;
    use chrono::TimeZone;
    use mockito::{Matcher, Server};

    /// Build an adapter whose API + token endpoints point at the mock
    /// server. The access token is valid for an hour so no refresh fires.
    fn adapter_for(server: &Server) -> GoogleAdapter {
        let mut adapter = GoogleAdapter::new(
            "client".into(),
            "secret".into(),
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

    #[tokio::test]
    async fn no_token_does_full_resync_and_returns_sync_token() {
        let mut server = Server::new_async().await;
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/calendars/primary/events\?.*timeMin=".to_string()),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"e1","summary":"One",
                     "start":{"dateTime":"2026-05-10T08:00:00Z"},
                     "end":{"dateTime":"2026-05-10T09:00:00Z"}}
                  ],
                  "nextSyncToken":"TOK-1"
                }"##,
            )
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_events_delta("primary", range(), None)
            .await
            .unwrap();
        assert!(cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert!(cs.deletions.is_empty());
        assert_eq!(cs.new_token.as_deref(), Some("TOK-1"));
    }

    #[tokio::test]
    async fn token_does_incremental_with_changes_and_deletions() {
        let mut server = Server::new_async().await;
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/calendars/primary/events\?.*syncToken=TOK-1".to_string()),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"e1","summary":"Updated",
                     "start":{"dateTime":"2026-05-10T08:00:00Z"},
                     "end":{"dateTime":"2026-05-10T09:00:00Z"}},
                    {"id":"e2","status":"cancelled",
                     "start":{"dateTime":"2026-05-11T08:00:00Z"},
                     "end":{"dateTime":"2026-05-11T09:00:00Z"}}
                  ],
                  "nextSyncToken":"TOK-2"
                }"##,
            )
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_events_delta("primary", range(), Some("TOK-1"))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(
            cs.changes.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["e1"]
        );
        assert_eq!(cs.deletions, vec!["e2".to_string()]);
        assert_eq!(cs.new_token.as_deref(), Some("TOK-2"));
    }

    #[tokio::test]
    async fn expired_token_410_falls_back_to_full_resync() {
        let mut server = Server::new_async().await;
        // Incremental request with the stale token → 410.
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/calendars/primary/events\?.*syncToken=STALE".to_string()),
            )
            .with_status(410)
            .with_body("Sync token is no longer valid")
            .create_async()
            .await;
        // Recovery: a full window sync with a fresh token.
        server
            .mock(
                "GET",
                Matcher::Regex(r"^/calendars/primary/events\?.*timeMin=".to_string()),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"e1","summary":"One",
                     "start":{"dateTime":"2026-05-10T08:00:00Z"},
                     "end":{"dateTime":"2026-05-10T09:00:00Z"}}
                  ],
                  "nextSyncToken":"TOK-3"
                }"##,
            )
            .create_async()
            .await;

        let cs = adapter_for(&server)
            .get_events_delta("primary", range(), Some("STALE"))
            .await
            .unwrap();
        // Transparent recovery: the caller sees a clean full resync.
        assert!(cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.new_token.as_deref(), Some("TOK-3"));
    }

    #[tokio::test]
    async fn contacts_delta_only_other_contacts_is_supported() {
        // The personal list couples contacts to group member lists and the
        // Directory is read-only — both must surface Unsupported (no network
        // call) so the host falls back to a full read.
        let server = Server::new_async().await;
        let adapter = adapter_for(&server);
        for list in [
            crate::contacts::GOOGLE_CONTACT_LIST_ID,
            crate::contacts::GOOGLE_DIRECTORY_LIST_ID,
        ] {
            let err = adapter.get_contacts_delta(list, None).await.unwrap_err();
            assert!(
                matches!(err, CoreError::Unsupported(_)),
                "{list} should be Unsupported",
            );
        }
    }
}
