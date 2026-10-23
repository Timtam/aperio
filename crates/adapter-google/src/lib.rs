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
mod series_create;
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

    /// The calendars, cached, as a [`GoogleError`]: a write that walks them
    /// maps a failed listing as a write, which wrote nothing.
    async fn calendars(&self) -> GoogleResult<Vec<Calendar>> {
        if let Some(cached) = self.cached_calendars().await {
            return Ok(cached);
        }
        let fresh = api::list_calendars(&self.state).await?;
        *self.calendars_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    /// The task lists, cached, as a [`GoogleError`] (see [`Self::calendars`]).
    async fn task_lists(&self) -> GoogleResult<Vec<TaskList>> {
        if let Some(cached) = self.cached_task_lists().await {
            return Ok(cached);
        }
        let fresh = tasks::list_task_lists(&self.state).await?;
        *self.task_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
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
        self.calendars().await.map_err(to_core_error)
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
            .map_err(to_create_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        api::update_event(&self.state, &event)
            .await
            .map_err(to_write_error)
    }

    async fn delete_event(&self, event_id: &str, send_cancellations: bool) -> CoreResult<()> {
        // An override id names one occurrence, and deleting it cancels that slot
        // (see the trait). It is no Google id, so the DELETE below could only
        // fail on every calendar.
        if let Some((master_id, slot)) = cal_core::split_override_id(event_id)? {
            return self
                .add_event_exdate(master_id, slot, send_cancellations)
                .await;
        }
        // Aperio's command layer hands us the calendar_id alongside the event_id
        // when it can, but the legacy `delete_event(event_id)` trait method doesn't
        // carry it. We walk every calendar and try the delete against each. A 404 =
        // "not on this calendar" (keep walking); a 410 = "already gone" anywhere =
        // success (idempotent). Any OTHER error (403/412/5xx) on a calendar is a
        // real failure — remember it and surface it after the walk instead of
        // masking it as the misleading "not found in any calendar".
        let cals = self.calendars().await.map_err(to_write_error)?;
        let mut real_error: Option<GoogleError> = None;
        for cal in cals {
            match api::delete_event(&self.state, &cal.id, event_id, send_cancellations).await {
                Ok(()) => return Ok(()),
                Err(GoogleError::Http { status: 404, .. }) => continue,
                Err(GoogleError::Http { status: 410, .. }) => return Ok(()),
                // An answer for the whole account stops the walk (decisions
                // 270, 275): the other calendars would be asked while it
                // holds, and a later answer would hide why.
                Err(e @ (GoogleError::Busy { .. } | GoogleError::UsageLimit { .. })) => {
                    return Err(stopped_walk(e, real_error.map(to_write_error)))
                }
                Err(e) => real_error = Some(e),
            }
        }
        if let Some(e) = real_error {
            return Err(to_write_error(e));
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
            .map_err(to_write_error)
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
            .map_err(to_write_error)?;
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
        let cals = self.calendars().await.map_err(to_write_error)?;
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
                Err(e) => return Err(to_write_error(e)),
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
        self.task_lists().await.map_err(to_core_error)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        tasks::get_tasks(&self.state, list_id)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> CoreResult<Task> {
        tasks::create_task(&self.state, list_id, task)
            .await
            .map_err(to_write_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        tasks::update_task(&self.state, &task)
            .await
            .map_err(to_write_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        // The trait signature drops the list_id, so we walk every
        // known list and let the first 2xx win. Mirrors the
        // `delete_event` fallback above — same trade-off (one extra
        // round-trip per non-matching list) and same justification:
        // the Aperio command layer already knows the list, but the
        // trait surface does not carry it.
        let lists = self.task_lists().await.map_err(to_write_error)?;
        // A list's answer that may have landed — a lost one — is kept: the
        // task may be gone, and "not found" would say nothing changed.
        // Whatever else a list answers, the task is looked for on the next.
        let mut unsure: Option<CoreError> = None;
        for list in lists {
            match tasks::delete_task(&self.state, &list.id, task_id).await {
                Ok(()) => return Ok(()),
                // An answer for the whole account stops the walk (decisions
                // 270, 275): every other list would be asked while it holds,
                // and "not found" would hide why.
                Err(e @ (GoogleError::Busy { .. } | GoogleError::UsageLimit { .. })) => {
                    return Err(stopped_walk(e, unsure))
                }
                Err(e) => {
                    let e = to_write_error(e);
                    if !e.never_landed() {
                        unsure = Some(e);
                    }
                }
            }
        }
        if let Some(e) = unsure {
            return Err(e);
        }
        Err(CoreError::NotFound(format!(
            "task '{task_id}' not found in any task list"
        )))
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        tasks::rename_task_list(&self.state, list_id, new_name)
            .await
            .map_err(to_write_error)?;
        // Drop the cached listing — the cached title is stale.
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }

    async fn create_task_list(&self, name: &str, parent_id: Option<&str>) -> CoreResult<TaskList> {
        let created = tasks::create_task_list(&self.state, name, parent_id)
            .await
            .map_err(to_write_error)?;
        *self.task_lists_cache.lock().await = None;
        Ok(created)
    }

    async fn delete_task_list(&self, list_id: &str) -> CoreResult<()> {
        tasks::delete_task_list(&self.state, list_id)
            .await
            .map_err(to_write_error)?;
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
        let result = contacts::create_contact(&self.state, contact).await;
        // Invalidate the listing cache so the next `get_contacts` shows the
        // newly created row — also one saved in part (decision 277).
        self.contacts_cache.lock().await.clear();
        result.map_err(to_write_error)
    }

    async fn update_contact(&self, contact: Contact) -> CoreResult<Contact> {
        if is_read_only_google_list(&contact.list_id) {
            return Err(CoreError::Unsupported(format!(
                "contacts in '{}' cannot be edited from Aperio",
                contact.list_id,
            )));
        }
        let result = contacts::update_contact(&self.state, contact).await;
        // Also after a change saved in part (decision 277).
        self.contacts_cache.lock().await.clear();
        result.map_err(to_write_error)
    }

    async fn delete_contact(&self, contact_id: &str) -> CoreResult<()> {
        contacts::delete_contact(&self.state, contact_id)
            .await
            .map_err(to_write_error)?;
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
            .map_err(to_write_error)?;
        self.contacts_cache.lock().await.clear();
        Ok(())
    }

    async fn delete_contact_photo(&self, contact_id: &str) -> CoreResult<()> {
        contacts::delete_contact_photo(&self.state, contact_id)
            .await
            .map_err(to_write_error)?;
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

/// [`to_core_error`] for an update. A status with which the server turned the
/// write down whole ([`cal_core::WriteRefusal::refused_status`]) is a refusal,
/// not a protocol error: a caller deciding whether the write may have landed —
/// splitting a series undoes its new part only when the cut certainly did not
/// (decision 144) — must be told that nothing was written.
fn to_write_error(err: GoogleError) -> CoreError {
    match err {
        // A used-up limit turned the write down whole: the server refused it,
        // with Google's reason (decision 275).
        GoogleError::UsageLimit { reason } => {
            tracing::warn!(%reason, "Google refused the write: a usage limit is used up");
            CoreError::Forbidden(cal_core::WriteRefusal::ServerRefused.message(&reason))
        }
        GoogleError::Http { status, message } if cal_core::WriteRefusal::refused_status(status) => {
            tracing::warn!(status, %message, "Google refused the update");
            CoreError::Forbidden(cal_core::WriteRefusal::ServerRefused.message(
                &match server_reason(&message) {
                    Some(reason) => format!("HTTP {status}: {reason}"),
                    None => format!("HTTP {status}"),
                },
            ))
        }
        other => to_core_error(other),
    }
}

/// What a walk over the calendars or lists says when an answer for the whole
/// account stopped it — a busy provider (decision 270), a used-up limit (275):
/// that answer, unless an earlier answer of the walk may have landed (a lost
/// one), which then stays what it was rather than read as "nothing changed".
fn stopped_walk(stop: GoogleError, earlier: Option<CoreError>) -> CoreError {
    match earlier {
        Some(earlier) if !earlier.never_landed() => earlier,
        _ => to_write_error(stop),
    }
}

/// A create's error (decisions 202-213, 270): a new series not kept is that
/// outcome, one Google turned down whole — a refused status, a busy provider,
/// a used-up limit — reads as the update's refusal, anything else as any
/// error.
fn to_create_error(err: GoogleError) -> CoreError {
    match err {
        err @ (GoogleError::NotKept { .. } | GoogleError::Busy { .. }) => to_core_error(err),
        err @ GoogleError::UsageLimit { .. } => to_write_error(err),
        GoogleError::Http { status, message } if cal_core::WriteRefusal::refused_status(status) => {
            to_write_error(GoogleError::Http { status, message })
        }
        other => to_core_error(other),
    }
}

/// The reason a JSON error answer gives (`{"error":{"message":"…"}}`),
/// trimmed, for the sentence that says the server refused: "HTTP 400" alone
/// tells the user nothing they can act on.
///
/// The error keeps only the first 300 characters of the answer, so a longer
/// envelope arrives cut and is no JSON any more. The first `"message"`
/// string is then read as far as it goes: it is the error's own, before the
/// details and the request ids that make an envelope long.
fn server_reason(body: &str) -> Option<String> {
    let reason = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => value.get("error")?.get("message")?.as_str()?.to_string(),
        Err(_) => {
            let rest = &body[body.find("\"message\"")? + "\"message\"".len()..];
            let rest = rest.trim_start().strip_prefix(':')?.trim_start();
            let rest = rest.strip_prefix('"')?;
            let mut out = String::new();
            let mut escaped = false;
            for c in rest.chars() {
                match (escaped, c) {
                    (true, _) => {
                        out.push(c);
                        escaped = false;
                    }
                    (false, '\\') => escaped = true,
                    (false, '"') => break,
                    (false, _) => out.push(c),
                }
            }
            out
        }
    };
    let reason = reason.trim();
    if reason.is_empty() {
        return None;
    }
    Some(reason.chars().take(160).collect())
}

/// A Google error as the core error it maps to, for the plugin's
/// interactive auth, which reports its failures as text
/// (`plugin_sdk::auth_error`).
pub fn core_error(err: GoogleError) -> CoreError {
    to_core_error(err)
}

fn to_core_error(err: GoogleError) -> CoreError {
    use GoogleError::*;
    match err {
        Network(m) | Unreached(m) => CoreError::Network(m),
        NotKept {
            undone: true,
            detail,
        } => CoreError::Forbidden(cal_core::WriteRefusal::DeletionsNotKept.message(&detail)),
        NotKept {
            undone: false,
            detail,
        } => CoreError::Network(cal_core::WriteLeftover::CreatedMayStand.message(&detail)),
        // Throttling: nothing was done, try later (decision 270).
        Busy {
            detail,
            retry_after,
        } => {
            tracing::debug!(detail = detail.word(), ?retry_after, "google is busy");
            CoreError::provider_busy(detail)
        }
        // A used-up limit: no sign-in helps (decision 275).
        UsageLimit { reason } => CoreError::Protocol(format!("google usage limit: {reason}")),
        // A later step failed after an earlier one landed (decision 277):
        // never a refusal, by the core's rule.
        PartlySaved { step, cause } => {
            tracing::debug!(step, %cause, "google saved the change in part");
            to_core_error(*cause).after_landed(step)
        }
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

    /// A series head, cut short: what splitting a series writes.
    fn truncated_master() -> Event {
        Event {
            original_left_in: None,
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "master-1".into(),
            calendar_id: "primary".into(),
            title: "Teamrunde".into(),
            description: None,
            location: None,
            start: chrono::Utc.with_ymd_and_hms(2026, 6, 1, 7, 0, 0).unwrap(),
            end: chrono::Utc.with_ymd_and_hms(2026, 6, 1, 8, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(cal_core::EventRecurrence {
                rrule: "FREQ=WEEKLY;UNTIL=20260824T065959Z".into(),
                exceptions: vec![],
                tzid: None,
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            etag: None,
            organizer: None,
            attendee_responses: vec![],
            send_invitations: false,
            truncate_tail_overrides: false,
            accepts_exception_loss: false,
            deletions_not_restored: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        }
    }

    /// Google read the truncate and turned it down whole: nothing was written,
    /// and the caller must know that for certain (decision 144), not as a
    /// protocol error it has to treat as maybe landed.
    #[tokio::test]
    async fn an_update_google_turned_down_is_a_refusal() {
        for status in [400] {
            let mut server = Server::new_async().await;
            let _patch = server
                .mock(
                    "PATCH",
                    Matcher::Regex(r"^/calendars/primary/events/master-1".into()),
                )
                .with_status(status)
                .with_body(r#"{"error":{"message":"no"}}"#)
                .create_async()
                .await;
            let err = adapter_for(&server)
                .update_event(truncated_master())
                .await
                .unwrap_err();
            // The server's own reason travels with the status.
            match err {
                CoreError::Forbidden(msg) => {
                    assert_eq!(msg, format!("server-refused: HTTP {status}: no"))
                }
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    /// A write or a read Google turns away for throttling says the provider
    /// is busy, and nothing was written (decision 270); a used-up limit no
    /// longer says to sign in again (275).
    #[tokio::test]
    async fn a_busy_google_says_so_on_a_write_and_on_a_read() {
        let busy = |reason: &str| {
            format!(
                r#"{{"error":{{"code":403,"errors":[{{"domain":"usageLimits","reason":"{reason}"}}]}}}}"#
            )
        };
        for (status, body, word) in [
            (429, r#"{"error":{"code":429}}"#.to_string(), "http-429"),
            (403, busy("rateLimitExceeded"), "rate-limit"),
            (403, busy("userRateLimitExceeded"), "user-rate-limit"),
        ] {
            let mut server = Server::new_async().await;
            let _patch = server
                .mock(
                    "PATCH",
                    Matcher::Regex(r"^/calendars/primary/events/master-1".into()),
                )
                .with_status(status)
                .with_body(&body)
                .create_async()
                .await;
            let _list = server
                .mock(
                    "GET",
                    Matcher::Regex(r"^/calendars/primary/events\?".into()),
                )
                .with_status(status)
                .with_body(&body)
                .create_async()
                .await;
            let adapter = adapter_for(&server);
            let err = adapter.update_event(truncated_master()).await.unwrap_err();
            assert!(err.is_provider_busy(), "{status} {err:?}");
            assert!(
                matches!(&err, CoreError::Forbidden(m) if m == &format!("provider-busy: {word}"))
            );
            let err = adapter.get_events("primary", range()).await.unwrap_err();
            assert!(err.is_provider_busy(), "{status} read {err:?}");
        }
        // A used-up limit: the write was refused by the server, with the
        // reason; a read is an ordinary failure. Neither says to sign in.
        let mut server = Server::new_async().await;
        let _patch = server
            .mock(
                "PATCH",
                Matcher::Regex(r"^/calendars/primary/events/master-1".into()),
            )
            .with_status(403)
            .with_body(busy("quotaExceeded"))
            .create_async()
            .await;
        let _list = server
            .mock(
                "GET",
                Matcher::Regex(r"^/calendars/primary/events\?".into()),
            )
            .with_status(403)
            .with_body(busy("dailyLimitExceeded"))
            .create_async()
            .await;
        let adapter = adapter_for(&server);
        match adapter.update_event(truncated_master()).await.unwrap_err() {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: quotaExceeded"),
            other => panic!("{other:?}"),
        }
        match adapter.get_events("primary", range()).await.unwrap_err() {
            CoreError::Protocol(m) => assert_eq!(m, "google usage limit: dailyLimitExceeded"),
            other => panic!("{other:?}"),
        }
        // Any other 403 keeps what it said.
        let mut server = Server::new_async().await;
        let _list = server
            .mock(
                "GET",
                Matcher::Regex(r"^/calendars/primary/events\?".into()),
            )
            .with_status(403)
            .with_body(busy("insufficientPermissions"))
            .create_async()
            .await;
        let err = adapter_for(&server)
            .get_events("primary", range())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Authentication(_)), "{err:?}");
    }

    /// A busy provider stops a walk over the calendars at once (decision
    /// 270): the delete and the answer to an invitation ask nothing more, and
    /// say the provider is busy rather than what a later calendar answered.
    #[tokio::test]
    async fn a_walk_over_the_calendars_stops_at_a_busy_provider() {
        let mut server = Server::new_async().await;
        let _list = server
            .mock("GET", "/users/me/calendarList")
            .with_status(200)
            .with_body(
                r##"{"items":[{"id":"work","summary":"Work","accessRole":"owner"},
                              {"id":"home","summary":"Home","accessRole":"owner"}]}"##,
            )
            .create_async()
            .await;
        let _primary = server
            .mock("GET", "/calendars/primary")
            .with_status(200)
            .with_body(r#"{"id":"me@example.com"}"#)
            .create_async()
            .await;
        let _first = server
            .mock(
                "DELETE",
                Matcher::Regex(r"^/calendars/work/events/ev1".into()),
            )
            .with_status(429)
            .create_async()
            .await;
        let second = server
            .mock("DELETE", Matcher::Regex(r"^/calendars/home/".into()))
            .expect(0)
            .create_async()
            .await;
        let adapter = adapter_for(&server);
        let err = adapter.delete_event("ev1", false).await.unwrap_err();
        assert!(err.is_provider_busy(), "{err:?}");
        second.assert_async().await;

        let _read = server
            .mock("GET", "/calendars/work/events/ev2")
            .with_status(403)
            .with_body(r#"{"error":{"code":403,"errors":[{"reason":"rateLimitExceeded"}]}}"#)
            .create_async()
            .await;
        let further = server
            .mock("GET", Matcher::Regex(r"^/calendars/home/events/".into()))
            .expect(0)
            .create_async()
            .await;
        let answer = server
            .mock("PATCH", Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let err = adapter
            .respond_to_event("ev2", AttendeeStatus::Accepted, false)
            .await
            .unwrap_err();
        assert!(err.is_provider_busy(), "{err:?}");
        further.assert_async().await;
        answer.assert_async().await;
    }

    /// A single write turned down for a used-up limit was refused by the
    /// server, with the reason, never a sign-in failure (decision 275).
    #[tokio::test]
    async fn a_write_refused_for_a_used_up_limit_says_the_server_refused() {
        let mut server = Server::new_async().await;
        let _rename = server
            .mock("PATCH", "/calendars/work")
            .with_status(403)
            .with_body(r#"{"error":{"code":403,"errors":[{"reason":"quotaExceeded"}]}}"#)
            .create_async()
            .await;
        match adapter_for(&server)
            .rename_calendar("work", "Arbeit")
            .await
            .unwrap_err()
        {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: quotaExceeded"),
            other => panic!("{other:?}"),
        }
    }

    /// A create Google turns down whole is a refusal like an update's
    /// (decisions 147, 270): a busy provider says so.
    #[tokio::test]
    async fn a_create_google_turned_down_is_a_refusal() {
        for (status, token) in [(400, "server-refused"), (429, "provider-busy")] {
            let mut server = Server::new_async().await;
            let _post = server
                .mock("POST", Matcher::Regex(r"^/calendars/cal/events\?".into()))
                .with_status(status)
                .with_body(r#"{"error":{"message":"no"}}"#)
                .create_async()
                .await;
            let err = adapter_for(&server)
                .create_event("cal", weekly_from_2030(vec![]))
                .await
                .unwrap_err();
            match &err {
                CoreError::Forbidden(m) => assert!(m.starts_with(token), "{status} {m}"),
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    /// A task or contact save Google turned down whole is a refusal like an
    /// event's (decisions 147, 270): nothing was changed.
    #[tokio::test]
    async fn a_task_or_contact_save_turned_down_whole_is_a_refusal() {
        let mut server = Server::new_async().await;
        let _list = server
            .mock("GET", Matcher::Regex(r"^/lists/L/tasks\?".into()))
            .with_status(200)
            .with_body(r#"{"items":[{"id":"T","title":"Einkaufen"}]}"#)
            .create_async()
            .await;
        let _patch = server
            .mock("PATCH", "/lists/L/tasks/T")
            .with_status(400)
            .with_body(r#"{"error":{"message":"bad due"}}"#)
            .create_async()
            .await;
        let _create = server
            .mock("POST", Matcher::Regex(r"^/people:createContact\?".into()))
            .with_status(400)
            .with_body(r#"{"error":{"message":"bad name"}}"#)
            .create_async()
            .await;
        let adapter = adapter_for(&server);
        let task = adapter.get_tasks("L").await.unwrap().remove(0);
        match adapter.update_task(task).await.unwrap_err() {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: HTTP 400: bad due"),
            other => panic!("{other:?}"),
        }
        let new: NewContact = serde_json::from_value(serde_json::json!({
            "display_name": "Max",
            "emails": [],
            "phone_numbers": [],
        }))
        .unwrap();
        match adapter
            .create_contact(contacts::GOOGLE_CONTACT_LIST_ID, new)
            .await
            .unwrap_err()
        {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: HTTP 400: bad name"),
            other => panic!("{other:?}"),
        }
        let _people = server
            .mock("GET", Matcher::Regex(r"^/people/me/connections\?".into()))
            .with_status(200)
            .with_body(r#"{"connections":[{"resourceName":"people/c1","etag":"e1","names":[{"displayName":"Max"}]}]}"#)
            .create_async()
            .await;
        let _groups = server
            .mock("GET", Matcher::Regex(r"^/contactGroups\?".into()))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let _events = server
            .mock(
                "GET",
                Matcher::Regex(r"^/people/c1\?personFields=events$".into()),
            )
            .with_status(200)
            .with_body(r#"{"resourceName":"people/c1"}"#)
            .create_async()
            .await;
        let _update = server
            .mock(
                "PATCH",
                Matcher::Regex(r"^/people/c1:updateContact\?".into()),
            )
            .with_status(400)
            .with_body(r#"{"error":{"message":"bad date"}}"#)
            .create_async()
            .await;
        let contact = adapter
            .get_contacts(contacts::GOOGLE_CONTACT_LIST_ID)
            .await
            .unwrap()
            .remove(0);
        match adapter.update_contact(contact).await.unwrap_err() {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: HTTP 400: bad date"),
            other => panic!("{other:?}"),
        }
    }

    /// A walk over the calendars stopped by a busy provider says so — unless
    /// an earlier calendar's delete lost its answer, which may have deleted
    /// the event: that stays unsure, never "nothing changed". A used-up
    /// limit stops it the same way, and a listing that fails before any
    /// delete is mapped as a write.
    #[tokio::test]
    async fn a_walk_stopped_for_the_account_keeps_an_earlier_unsure_answer() {
        let calendars =
            r#"{"items":[{"id":"work","summary":"Work"},{"id":"home","summary":"Home"}]}"#;
        let busy = r#"{"error":{"code":429}}"#;
        // work: lost answer (500); home: busy.
        let mut server = Server::new_async().await;
        let _list = server
            .mock("GET", Matcher::Regex(r"^/users/me/calendarList".into()))
            .with_status(200)
            .with_body(calendars)
            .create_async()
            .await;
        let _work = server
            .mock(
                "DELETE",
                Matcher::Regex(r"^/calendars/work/events/ev1".into()),
            )
            .with_status(500)
            .create_async()
            .await;
        let _home = server
            .mock(
                "DELETE",
                Matcher::Regex(r"^/calendars/home/events/ev1".into()),
            )
            .with_status(429)
            .with_body(busy)
            .create_async()
            .await;
        let err = adapter_for(&server)
            .delete_event("ev1", false)
            .await
            .unwrap_err();
        assert!(!err.never_landed(), "{err:?}");
        assert!(!err.is_provider_busy(), "{err:?}");
        // work: a used-up limit, which holds for the whole account: home is
        // not asked.
        let mut server = Server::new_async().await;
        let _list = server
            .mock("GET", Matcher::Regex(r"^/users/me/calendarList".into()))
            .with_status(200)
            .with_body(calendars)
            .create_async()
            .await;
        let _work = server
            .mock(
                "DELETE",
                Matcher::Regex(r"^/calendars/work/events/ev1".into()),
            )
            .with_status(403)
            .with_body(r#"{"error":{"code":403,"errors":[{"reason":"dailyLimitExceeded"}]}}"#)
            .create_async()
            .await;
        let home = server
            .mock(
                "DELETE",
                Matcher::Regex(r"^/calendars/home/events/ev1".into()),
            )
            .expect(0)
            .create_async()
            .await;
        match adapter_for(&server)
            .delete_event("ev1", false)
            .await
            .unwrap_err()
        {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: dailyLimitExceeded"),
            other => panic!("{other:?}"),
        }
        home.assert_async().await;
        // The listing itself turned down: nothing was written.
        let mut server = Server::new_async().await;
        let _list = server
            .mock("GET", Matcher::Regex(r"^/users/me/calendarList".into()))
            .with_status(403)
            .with_body(r#"{"error":{"code":403,"errors":[{"reason":"quotaExceeded"}]}}"#)
            .create_async()
            .await;
        match adapter_for(&server)
            .delete_event("ev1", false)
            .await
            .unwrap_err()
        {
            CoreError::Forbidden(m) => assert_eq!(m, "server-refused: quotaExceeded"),
            other => panic!("{other:?}"),
        }
    }

    /// A task delete whose list lost its answer stays unsure; one no list
    /// holds is not found.
    #[tokio::test]
    async fn a_task_delete_that_lost_its_answer_is_unsure() {
        let lists = r#"{"items":[{"id":"L1","title":"One"},{"id":"L2","title":"Two"}]}"#;
        for (first, unsure) in [(500, true), (404, false)] {
            let mut server = Server::new_async().await;
            let _lists = server
                .mock("GET", Matcher::Regex(r"^/users/@me/lists".into()))
                .with_status(200)
                .with_body(lists)
                .create_async()
                .await;
            let _one = server
                .mock("DELETE", "/lists/L1/tasks/T")
                .with_status(first)
                .create_async()
                .await;
            let _two = server
                .mock("DELETE", "/lists/L2/tasks/T")
                .with_status(404)
                .create_async()
                .await;
            let err = adapter_for(&server).delete_task("T").await.unwrap_err();
            if unsure {
                assert!(!err.never_landed(), "{err:?}");
            } else {
                assert!(matches!(err, CoreError::NotFound(_)), "{err:?}");
            }
        }
    }

    /// A save stopped after an earlier step landed is never a refusal
    /// (decision 277): a busy provider has its sentence, any other cause is
    /// an error read as "may have landed".
    #[test]
    fn a_save_left_in_part_is_never_a_refusal() {
        let in_part = |step, cause| GoogleError::PartlySaved {
            step,
            cause: Box::new(cause),
        };
        let busy = GoogleError::Busy {
            detail: cal_core::BusyDetail::TooManyRequests,
            retry_after: None,
        };
        match to_write_error(in_part("members", busy)) {
            CoreError::Network(m) => assert_eq!(
                cal_core::WriteLeftover::parse(&m),
                Some((cal_core::WriteLeftover::PartlySaved, "members"))
            ),
            other => panic!("{other:?}"),
        }
        for cause in [
            GoogleError::Http {
                status: 400,
                message: "no".into(),
            },
            GoogleError::UsageLimit {
                reason: "quotaExceeded".into(),
            },
            GoogleError::Http {
                status: 401,
                message: "no".into(),
            },
        ] {
            match to_write_error(in_part("move", cause)) {
                CoreError::Network(m) => {
                    assert!(m.starts_with("move: "), "{m}");
                    assert_eq!(cal_core::WriteLeftover::parse(&m), None);
                    assert_eq!(cal_core::WriteRefusal::parse(&m), None);
                }
                other => panic!("{other:?}"),
            }
        }
        // A lost answer stays one.
        assert!(matches!(
            to_write_error(in_part("move", GoogleError::Network("reset".into()))),
            CoreError::Network(m) if m == "reset"
        ));
    }

    /// A contact saved in part is read fresh: the listing cache is dropped.
    #[tokio::test]
    async fn a_contact_saved_in_part_is_read_fresh() {
        let mut server = Server::new_async().await;
        let _search = server
            .mock("GET", Matcher::Regex(r"^/people:searchContacts\?".into()))
            .with_status(200)
            .with_body(
                r#"{"results":[{"person":{"resourceName":"people/c1","emailAddresses":[{"value":"a@x.de"}]}}]}"#,
            )
            .create_async()
            .await;
        let _others = server
            .mock(
                "GET",
                Matcher::Regex(r"^/(otherContacts:search|people:searchDirectoryPeople)\?".into()),
            )
            .with_status(403)
            .create_async()
            .await;
        let _created = server
            .mock("POST", "/contactGroups")
            .with_status(200)
            .with_body(r#"{"resourceName":"contactGroups/g1","name":"Friends"}"#)
            .create_async()
            .await;
        let _modified = server
            .mock("POST", "/contactGroups/g1/members:modify")
            .with_status(429)
            .create_async()
            .await;
        let adapter = adapter_for(&server);
        adapter.contacts_cache.lock().await.insert(
            contacts::GOOGLE_CONTACT_LIST_ID.into(),
            (Vec::new(), chrono::Utc::now()),
        );
        let new: NewContact = serde_json::from_value(serde_json::json!({
            "display_name": "Friends",
            "emails": [],
            "phone_numbers": [],
            "members": [{"email": "a@x.de"}],
        }))
        .unwrap();
        match adapter
            .create_contact(contacts::GOOGLE_CONTACT_LIST_ID, new)
            .await
            .unwrap_err()
        {
            CoreError::Network(m) => assert_eq!(m, "partly-saved: members"),
            other => panic!("{other:?}"),
        }
        assert!(adapter.contacts_cache.lock().await.is_empty());
    }

    /// The error keeps only the first 300 characters of an answer, so a
    /// realistic envelope arrives cut: its reason is still read.
    #[test]
    fn a_series_not_kept_is_a_refusal_or_a_leftover() {
        // Deleted again: nothing stands, a refusal the shells read as such.
        match to_core_error(GoogleError::NotKept {
            undone: true,
            detail: "invited time".into(),
        }) {
            CoreError::Forbidden(msg) => assert_eq!(
                cal_core::WriteRefusal::parse(&msg),
                Some((cal_core::WriteRefusal::DeletionsNotKept, "invited time"))
            ),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // Not deleted again: it may stand, never a refusal.
        match to_core_error(GoogleError::NotKept {
            undone: false,
            detail: "create-answer-lost".into(),
        }) {
            CoreError::Network(msg) => assert_eq!(
                cal_core::WriteLeftover::parse(&msg),
                Some((
                    cal_core::WriteLeftover::CreatedMayStand,
                    "create-answer-lost"
                ))
            ),
            other => panic!("expected a leftover, got {other:?}"),
        }
        // A request that never left is a plain network error.
        assert!(matches!(
            to_core_error(GoogleError::Unreached("refused".into())),
            CoreError::Network(_)
        ));
    }

    #[test]
    fn the_reason_is_read_from_a_cut_answer() {
        let long = "The recurrence rule does not generate an occurrence on the start date \
                    of the event, so the event cannot be saved as it is.";
        let body = format!(
            r#"{{"error":{{"code":400,"message":"{long}","errors":[{{"domain":"global","reason":"invalid","message":"{long}"}}],"innerError":{{"date":"2026-09-26T10:00:00","request-id":"0f6c1d2e-9a8b-4c3d-8e7f-6a5b4c3d2e1f"}}}}}}"#
        );
        let cut: String = body.chars().take(300).collect();
        assert!(
            serde_json::from_str::<serde_json::Value>(&cut).is_err(),
            "the test must cut"
        );
        assert_eq!(server_reason(&cut).as_deref(), Some(long));
        // A reason cut itself is read as far as it goes.
        let cut_early: String = body.chars().take(60).collect();
        assert!(server_reason(&cut_early).is_some_and(|r| long.starts_with(&r)));
        // No message, no reason.
        assert_eq!(
            server_reason(r#"{"error":{"code":"ErrorInvalidRequest"}}"#),
            None
        );
        assert_eq!(server_reason("<html>Bad Request</html>"), None);
    }

    const PATCH_PATH: &str = r"^/calendars/primary/events/master-1";

    /// The save met an expired access token, and the token endpoint refused
    /// the refresh (a revoked grant answers 400 `invalid_grant`): a sign-in
    /// failure, as the error the user can act on — not the calendar server
    /// refusing the change.
    #[tokio::test]
    async fn a_refused_token_refresh_during_an_update_is_a_sign_in_failure() {
        let mut server = Server::new_async().await;
        let _write = server
            .mock("PATCH", Matcher::Regex(PATCH_PATH.into()))
            .with_status(401)
            .create_async()
            .await;
        let _token = server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#)
            .create_async()
            .await;
        let err = adapter_for(&server)
            .update_event(truncated_master())
            .await
            .unwrap_err();
        match err {
            CoreError::Authentication(msg) => assert!(msg.contains("invalid_grant"), "{msg}"),
            other => panic!("expected a sign-in failure, got {other:?}"),
        }
    }

    /// A server error says nothing about what was written: unsure, as before.
    #[tokio::test]
    async fn an_update_that_failed_in_the_server_is_not_a_refusal() {
        let mut server = Server::new_async().await;
        let _patch = server
            .mock(
                "PATCH",
                Matcher::Regex(r"^/calendars/primary/events/master-1".into()),
            )
            .with_status(503)
            .create_async()
            .await;
        let err = adapter_for(&server)
            .update_event(truncated_master())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Protocol(_)), "{err:?}");
    }

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
        adapter.state.people_base = server.url();
        adapter.state.tasks_base = server.url();
        adapter.state.token_url = format!("{}/token", server.url());
        adapter
    }

    /// A weekly series on Mondays 09:00 UTC from 7 January 2030, with `deleted`.
    fn weekly_from_2030(deleted: Vec<chrono::DateTime<chrono::Utc>>) -> NewEvent {
        NewEvent {
            title: "Teamrunde".into(),
            description: None,
            location: None,
            start: chrono::Utc.with_ymd_and_hms(2030, 1, 7, 9, 0, 0).unwrap(),
            end: chrono::Utc.with_ymd_and_hms(2030, 1, 7, 10, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(cal_core::EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO;COUNT=4".into(),
                exceptions: deleted,
                tzid: None,
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
            organizer: None,
            organized_elsewhere: false,
        }
    }

    /// Through the adapter: a series with a deletion ahead is created bare
    /// under an id of Aperio's, its occurrences listed, the deleted one
    /// cancelled, and the listing read again (decisions 202, 268).
    #[tokio::test]
    async fn a_new_series_with_a_deletion_ahead_is_created_bare_and_cancelled() {
        let mut server = Server::new_async().await;
        let sent = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink = std::sync::Arc::clone(&sent);
        let insert = server
            .mock("POST", "/calendars/cal/events?sendUpdates=none")
            .with_status(200)
            .with_body_from_request(move |request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body().unwrap()).unwrap();
                *sink.lock().unwrap() = body.clone();
                serde_json::json!({
                    "id": body["id"],
                    "status": "confirmed",
                    "summary": body["summary"],
                    "start": body["start"],
                    "end": body["end"],
                    "recurrence": body["recurrence"],
                })
                .to_string()
                .into_bytes()
            })
            .expect(1)
            .create_async()
            .await;
        let seen = std::sync::Arc::clone(&cancelled);
        let list = server
            .mock(
                "GET",
                Matcher::Regex(
                    r"^/calendars/cal/events/[0-9a-f]{32}/instances\?showDeleted=true&timeMin="
                        .into(),
                ),
            )
            .with_status(200)
            .with_body_from_request(move |request| {
                let series = request.path().split('/').nth(4).unwrap().to_string();
                let gone = seen.load(std::sync::atomic::Ordering::SeqCst);
                let item = |day: u32, cancelled: bool| {
                    serde_json::json!({
                        "id": format!("{series}_203001{day:02}T090000Z"),
                        "status": if cancelled { "cancelled" } else { "confirmed" },
                        "recurringEventId": series,
                        "originalStartTime": { "dateTime": format!("2030-01-{day:02}T09:00:00Z") },
                    })
                };
                serde_json::json!({ "items": [item(7, false), item(14, gone), item(21, false)] })
                    .to_string()
                    .into_bytes()
            })
            .expect(2)
            .create_async()
            .await;
        let done = std::sync::Arc::clone(&cancelled);
        let patch = server
            .mock(
                "PATCH",
                Matcher::Regex(
                    r"^/calendars/cal/events/[0-9a-f]{32}_20300114T090000Z\?sendUpdates=none$"
                        .into(),
                ),
            )
            .match_body(Matcher::Json(serde_json::json!({ "status": "cancelled" })))
            .with_status(200)
            .with_body_from_request(move |request| {
                done.store(true, std::sync::atomic::Ordering::SeqCst);
                let id = request.path().rsplit('/').next().unwrap().to_string();
                serde_json::json!({ "id": id, "status": "cancelled" })
                    .to_string()
                    .into_bytes()
            })
            .expect(1)
            .create_async()
            .await;

        let deleted = chrono::Utc.with_ymd_and_hms(2030, 1, 14, 9, 0, 0).unwrap();
        let created = adapter_for(&server)
            .create_event("cal", weekly_from_2030(vec![deleted]))
            .await
            .unwrap();
        insert.assert_async().await;
        list.assert_async().await;
        patch.assert_async().await;
        let sent = sent.lock().unwrap().clone();
        let id = sent["id"].as_str().unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(
            sent["recurrence"],
            serde_json::json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4"])
        );
        assert_eq!(created.recurrence.unwrap().exceptions, [deleted]);
    }

    /// A series without deletions, or with deletions only in the past, is one
    /// create as any (263): nothing listed, nothing cancelled.
    #[tokio::test]
    async fn a_new_series_with_nothing_to_cancel_is_one_create() {
        let ahead = chrono::Utc.with_ymd_and_hms(2030, 1, 14, 9, 0, 0).unwrap();
        let past = chrono::Utc.with_ymd_and_hms(2020, 1, 6, 9, 0, 0).unwrap();
        // No deletion, one in the past, and one ahead without a rule: no
        // series to write in steps, as the surfaces' wait says too.
        for (deleted, rule) in [(vec![], true), (vec![past], true), (vec![ahead], false)] {
            let mut server = Server::new_async().await;
            let sent = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
            let sink = std::sync::Arc::clone(&sent);
            let insert = server
                .mock("POST", "/calendars/cal/events?sendUpdates=none")
                .with_status(200)
                .with_body_from_request(move |request| {
                    let body: serde_json::Value =
                        serde_json::from_slice(request.body().unwrap()).unwrap();
                    *sink.lock().unwrap() = body.clone();
                    serde_json::json!({
                        "id": "plain1",
                        "status": "confirmed",
                        "start": body["start"],
                        "end": body["end"],
                        "recurrence": body["recurrence"],
                    })
                    .to_string()
                    .into_bytes()
                })
                .expect(1)
                .create_async()
                .await;
            let elsewhere = server
                .mock("GET", Matcher::Any)
                .expect(0)
                .create_async()
                .await;
            let mut event = weekly_from_2030(deleted.clone());
            if !rule {
                event.recurrence.as_mut().unwrap().rrule = String::new();
            }
            let created = adapter_for(&server)
                .create_event("cal", event)
                .await
                .unwrap();
            insert.assert_async().await;
            elsewhere.assert_async().await;
            let sent = sent.lock().unwrap().clone();
            assert!(sent.get("id").is_none(), "{sent}");
            if rule {
                assert!(!sent.to_string().contains("EXDATE"), "{sent}");
            }
            assert_eq!(created.id, "plain1");
        }
    }

    fn range() -> DateRange {
        DateRange::new(
            chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
        )
    }

    /// Moving a changed occurrence to another calendar deletes it from the
    /// source by its override id, which is no Google id: the DELETE failed on
    /// every calendar and the moved occurrence stayed behind as a duplicate.
    #[tokio::test]
    async fn deleting_an_override_cancels_the_instance_in_its_slot() {
        let mut server = Server::new_async().await;
        server
            .mock("GET", "/users/me/calendarList")
            .with_status(200)
            .with_body(r##"{"items":[{"id":"primary","summary":"Me","accessRole":"owner"}]}"##)
            .create_async()
            .await;
        server
            .mock("GET", "/calendars/primary/events/master-1")
            .with_status(200)
            .with_body(
                r##"{"id":"master-1","start":{"dateTime":"2026-05-25T18:00:00Z"},
                     "end":{"dateTime":"2026-05-25T19:00:00Z"},
                     "recurrence":["RRULE:FREQ=WEEKLY"]}"##,
            )
            .create_async()
            .await;
        server
            .mock(
                "GET",
                "/calendars/primary/events/master-1/instances\
                 ?showDeleted=true&originalStart=2026-06-01T18%3A00%3A00Z",
            )
            .with_status(200)
            .with_body(
                r##"{"items":[
                  {"id":"master-1_20260601T180000Z","status":"confirmed",
                   "originalStartTime":{"dateTime":"2026-06-01T18:00:00Z"}}
                ]}"##,
            )
            .create_async()
            .await;
        let cancel = server
            .mock(
                "PATCH",
                "/calendars/primary/events/master-1_20260601T180000Z?sendUpdates=none",
            )
            .match_body(Matcher::Regex(r#""status":"cancelled""#.to_string()))
            .with_status(200)
            .with_body(r#"{"id":"master-1_20260601T180000Z","status":"cancelled"}"#)
            .create_async()
            .await;
        let delete = server
            .mock("DELETE", Matcher::Any)
            .expect(0)
            .create_async()
            .await;

        adapter_for(&server)
            .delete_event("master-1::rid::2026-06-01T18:00:00Z", false)
            .await
            .unwrap();
        cancel.assert_async().await;
        delete.assert_async().await;
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
