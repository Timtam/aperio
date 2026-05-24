//! CalDAV calendar + task adapter (DESIGN.md §6.2).
//!
//! Phase 6b lands in three internal increments:
//!
//!  - **6b.1** (this iteration) — server discovery + auth. The
//!    adapter can take a hostname / URL + credentials and find the
//!    URL of the user's calendar-home collection. That is the
//!    foundation every following operation builds on.
//!  - **6b.2** — calendar listing + range read of events.
//!  - **6b.3** — create / update / delete events + VTODO tasks.
//!
//! The shape is intentionally small for now: one `CaldavAdapter`
//! struct, constructed from `Credentials`, holding a shared
//! `reqwest::Client` and the discovered URLs after the first call to
//! [`CaldavAdapter::discover`]. Implementing the full `cal-core`
//! `Adapter` / `CalendarFeature` / `TasksFeature` trait surface is
//! left to the later iterations — at this point the registry layer
//! that would route through those traits doesn't exist yet either.

mod auth;
pub mod calendars;
pub mod config;
pub mod contacts;
pub mod discovery;
pub mod error;
pub mod events;
pub mod mapping;
pub mod tasks;
pub mod vcard;
mod xml;

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, Contact, ContactList,
    ContactsFeature, ContainerColor, Credentials as CoreCredentials, DateRange,
    Error as CoreError, Event, FreeBusy, NewContact, NewEvent, NewTask,
    Result as CoreResult, Task, TaskList, TasksFeature,
};
use reqwest::Client;
use url::Url;

pub use config::{AuthKind, CaldavAccountConfig, Credentials};
pub use discovery::Discovery;
pub use error::{CaldavError, CaldavResult};

/// Cached listing result with the timestamp it was fetched at.
/// Returned to the trait impl in cloned form so the lock can be
/// released before any await point.
#[derive(Debug, Clone)]
struct ListingCache<T> {
    items: Vec<T>,
    cached_at: chrono::DateTime<chrono::Utc>,
}

/// One configured CalDAV account. Cheap to clone — the `Client`s
/// are reference-counted by reqwest and the `Mutex`es only protect
/// the lazily-cached discovery + listing results.
pub struct CaldavAdapter {
    credentials: Credentials,
    /// Used for every "real" CalDAV operation (PROPFIND on
    /// calendar-home, PROPPATCH, REPORT, PUT, DELETE, …). Follows
    /// HTTP redirects up to 5 hops so a server's transparent move
    /// (Nextcloud reverse-proxy reshuffle, iCloud route changes)
    /// doesn't break the user.
    http: Client,
    /// Discovery-only client. The well-known step
    /// (`/.well-known/caldav`) is supposed to land on a 3xx whose
    /// `Location:` header carries the actual DAV root — and
    /// `discovery::resolve_well_known` needs to *see* that
    /// redirect rather than have reqwest swallow it, otherwise it
    /// would chase the 3xx with a GET and most CalDAV servers
    /// answer those with 405 / 501. Kept on a separate Client so
    /// the regular Client can follow redirects safely.
    http_no_redirect: Client,
    /// Filled on first `discover()` call so subsequent reads don't
    /// re-walk the well-known chain. Cleared when the user changes
    /// credentials (currently by constructing a fresh adapter).
    discovery: Mutex<Option<Discovery>>,
    /// Cached calendar listing. PROPFIND on the calendar-home is the
    /// expensive part — typically 500 ms–2 s against iCloud — and a
    /// freshly-mounted sidebar can trigger it three or four times in
    /// a row through the refresh paths.
    calendars_cache: Mutex<Option<ListingCache<Calendar>>>,
    /// Same idea for VTODO collections.
    task_lists_cache: Mutex<Option<ListingCache<TaskList>>>,
    /// And for CardDAV address books. The trait impl bails with an
    /// empty Vec when discovery didn't surface an
    /// `addressbook-home-set` — see `addressbook_home_url`
    /// handling below.
    contact_lists_cache: Mutex<Option<ListingCache<ContactList>>>,
    /// Freshness window for the two listing caches. Default 5 min —
    /// calendars are rarely added/removed/renamed, so this trades a
    /// short staleness window (a calendar created on iCloud's web
    /// UI shows up within 5 min) for snappy renames and account adds
    /// here in Aperio. The `rename_*` paths explicitly invalidate.
    listing_ttl: chrono::Duration,
    /// Capabilities the adapter declares to the registry. Always
    /// includes Calendar + Tasks (every CalDAV collection is a
    /// candidate for VEVENT or VTODO and the trait-level methods
    /// degrade gracefully when a server offers neither). Contacts
    /// is always advertised too — the trait's `list_contact_lists`
    /// returns an empty Vec when discovery didn't find an
    /// addressbook-home-set, so a calendar-only server still shows
    /// up correctly with zero contact books.
    capabilities: Vec<Capability>,
}

impl CaldavAdapter {
    /// Construct an adapter for a single account.
    ///
    /// `connect_timeout` defaults to 10s when `None` — most CalDAV
    /// servers respond within that window and the user should not
    /// be left waiting on a typo'd URL for the full system default.
    pub fn new(
        credentials: Credentials,
        connect_timeout: Option<Duration>,
    ) -> CaldavResult<Self> {
        let connect = connect_timeout.unwrap_or(Duration::from_secs(10));
        // Production client: follow redirects up to 5 hops. CalDAV
        // PROPFIND / PROPPATCH / REPORT / PUT / DELETE on a moved
        // collection should land on the new URL transparently
        // instead of failing the request.
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(connect)
            .timeout(Duration::from_secs(30))
            .build()?;
        // Discovery client: never auto-follow. The well-known
        // discovery step needs to see the 301/302 directly to read
        // the `Location:` header — letting reqwest chase it would
        // land us on the final endpoint with a GET, which most
        // CalDAV servers answer with 405 / 501.
        let http_no_redirect = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(connect)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            credentials,
            http,
            http_no_redirect,
            discovery: Mutex::new(None),
            calendars_cache: Mutex::new(None),
            task_lists_cache: Mutex::new(None),
            contact_lists_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
            capabilities: vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts,
            ],
        })
    }

    /// Override the listing-cache freshness window. Production
    /// callers use the 5-min default; tests inject a zero TTL when
    /// they want to verify the network-fetch path runs every time.
    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.listing_ttl = chrono::Duration::from_std(ttl)
            .unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    /// Drop both listing caches. Called from `rename_*` after a
    /// successful PROPPATCH so the next `list_*` call walks the
    /// network and surfaces the new displayname. Public for the
    /// unlikely future case of "the user knows the server changed
    /// out-of-band" — currently only the rename paths invoke it.
    pub fn invalidate_listing_caches(&self) {
        *self.calendars_cache.lock().expect("poison") = None;
        *self.task_lists_cache.lock().expect("poison") = None;
        *self.contact_lists_cache.lock().expect("poison") = None;
    }

    /// Return the cached calendars if any are within the TTL window.
    /// Pulled out as a helper so the trait impl below stays readable
    /// and the mutex guard is dropped before the await point.
    fn cached_calendars(&self) -> Option<Vec<Calendar>> {
        let guard = self.calendars_cache.lock().expect("poison");
        let entry = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(entry.cached_at);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(entry.items.clone())
        } else {
            None
        }
    }

    fn cached_task_lists(&self) -> Option<Vec<TaskList>> {
        let guard = self.task_lists_cache.lock().expect("poison");
        let entry = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(entry.cached_at);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(entry.items.clone())
        } else {
            None
        }
    }

    fn cached_contact_lists(&self) -> Option<Vec<ContactList>> {
        let guard = self.contact_lists_cache.lock().expect("poison");
        let entry = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(entry.cached_at);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(entry.items.clone())
        } else {
            None
        }
    }

    /// Pick a URL on the contact server that the photo-CRUD
    /// helpers can join contact resource paths against. The
    /// `ContactsFeature` photo methods take only `contact_id`
    /// (which encodes the resource href), so we need a base URL
    /// from the same host for `Url::join` to resolve absolute
    /// paths against. Any address-book URL works; the home URL
    /// is preferred because discovery already cached it.
    async fn contact_resource_base(&self) -> CoreResult<Url> {
        let discovery = self.discover().await.map_err(to_core_error)?;
        discovery.addressbook_home_url.clone().ok_or_else(|| {
            CoreError::NotFound(
                "no addressbook-home-set; this server does not advertise CardDAV"
                    .into(),
            )
        })
    }

    /// Returns the discovered calendar-home URL, running the
    /// discovery chain once and caching the result. Subsequent calls
    /// return the cached value without any HTTP traffic.
    pub async fn discover(&self) -> CaldavResult<Discovery> {
        {
            if let Some(d) = self.discovery.lock().expect("poison").as_ref() {
                return Ok(d.clone());
            }
        }
        // Discovery uses the no-redirect client so the well-known
        // chain's 3xx can be read directly.
        let fresh = discovery::run(&self.http_no_redirect, &self.credentials).await?;
        *self.discovery.lock().expect("poison") = Some(fresh.clone());
        Ok(fresh)
    }

    /// Re-run discovery from scratch. Used when the user changes
    /// their server URL or after they re-enter credentials.
    pub async fn refresh_discovery(&self) -> CaldavResult<Discovery> {
        *self.discovery.lock().expect("poison") = None;
        self.discover().await
    }

    /// Test-only: peek at the cached result without going to the wire.
    #[cfg(test)]
    fn cached_calendar_home(&self) -> Option<url::Url> {
        self.discovery
            .lock()
            .expect("poison")
            .as_ref()
            .map(|d| d.calendar_home_url.clone())
    }
}

/// Translate a CalDAV-specific error into the shared `cal_core::Error`
/// shape so the rest of the app can pattern-match it uniformly.
fn to_core_error(err: CaldavError) -> CoreError {
    match err {
        CaldavError::Network(msg) => CoreError::Network(msg),
        CaldavError::Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("HTTP {status}: {message}")),
        },
        CaldavError::Protocol(msg) => CoreError::Protocol(msg),
        CaldavError::Discovery(msg) => CoreError::Protocol(format!("discovery: {msg}")),
        CaldavError::Config(msg) => CoreError::InvalidInput(msg),
    }
}

#[async_trait]
impl Adapter for CaldavAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
        // CalDAV auth happens per request via the Authorization
        // header — there's no separate token to fetch up front.
        // Triggering a discovery here doubles as a connection +
        // credential smoke test so the registry can decide whether
        // to keep the account active.
        self.discover().await.map_err(to_core_error)?;
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for CaldavAdapter {
    async fn list_calendars(&self) -> CoreResult<Vec<Calendar>> {
        if let Some(cached) = self.cached_calendars() {
            return Ok(cached);
        }
        let discovery = self.discover().await.map_err(to_core_error)?;
        let fresh = calendars::list_calendars(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        *self.calendars_cache.lock().expect("poison") = Some(ListingCache {
            items: fresh.clone(),
            cached_at: chrono::Utc::now(),
        });
        Ok(fresh)
    }

    async fn get_events(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> CoreResult<Vec<Event>> {
        // The calendar id is the absolute collection URL produced
        // by `list_calendars`. Re-parse it so the request lands at
        // the exact path the server told us about; falling back to
        // a join against the discovered home would be too lax.
        let cal_url = Url::parse(calendar_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        events::get_events(&self.http, &cal_url, range, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn create_event(
        &self,
        calendar_id: &str,
        event: NewEvent,
    ) -> CoreResult<Event> {
        let cal_url = Url::parse(calendar_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        events::create_event(&self.http, &cal_url, event, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        events::update_event(&self.http, event, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        // Same walk-the-home-set workaround as delete_event below —
        // the trait signature loses the parent calendar id, so we
        // try every calendar in the home set until one accepts the
        // EXDATE update. The aperio command layer routes via the
        // registry's calendar→account map, so production hits this
        // path with the right adapter already; the walk is the
        // fallback if a caller forgot to thread the calendar_id
        // through.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let cals = calendars::list_calendars(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        let mut last_err: Option<CoreError> = None;
        for cal in cals {
            let cal_url = match Url::parse(&cal.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            match events::add_event_exdate(
                &self.http,
                &cal_url,
                event_id,
                occurrence,
                &self.credentials,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // 404 just means "this event lives in a different
                    // calendar" — keep walking. Anything else we
                    // remember in case nothing else works.
                    if matches!(err, CaldavError::Http { status: 404, .. }) {
                        continue;
                    }
                    last_err = Some(to_core_error(err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            CoreError::NotFound(format!(
                "event '{event_id}' not found in any calendar"
            ))
        }))
    }

    async fn delete_event(&self, event_id: &str) -> CoreResult<()> {
        // The trait signature only gives us the event id. CalDAV
        // needs the calendar collection URL too — we recover it
        // by re-reading the discovery cache; callers that know
        // the calendar URL up front can hit `events::delete_event`
        // directly. The current API is good enough for the
        // registry layer where the calling code picked up the
        // event from `get_events` first (so we know which calendar
        // it lives on).
        //
        // Without the calendar id we fall back to a best-effort:
        // walk every calendar in the home set and try to DELETE on
        // each. That's the lazy path; the registry will refactor
        // the trait signature in Phase 6b.4 to thread the calendar
        // id through delete as well.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let cals = calendars::list_calendars(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        for cal in cals {
            let cal_url = match Url::parse(&cal.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            // Without an ETag we don't bother with If-Match — the
            // user explicitly chose to delete this row, so a
            // concurrent modification is informational at best.
            if let Ok(()) = events::delete_event(
                &self.http,
                &cal_url,
                event_id,
                None,
                &self.credentials,
            )
            .await
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
        // CalDAV exposes free-busy at the principal level via a
        // separate REPORT. Out of scope for the calendar-first
        // iteration; returning an empty list keeps consumers calm.
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // The color is fetched together with the listing and lives
        // on the Calendar struct already. A second per-id round-trip
        // would just duplicate work; consumers should use the value
        // off the Calendar they got from `list_calendars`.
        None
    }

    async fn rename_calendar(
        &self,
        calendar_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        // Calendar id == collection URL in CalDAV (see `to_calendar`).
        let url = Url::parse(calendar_id).map_err(|e| {
            CoreError::InvalidInput(format!("calendar id is not a URL: {e}"))
        })?;
        calendars::proppatch_displayname(&self.http, &url, new_name, &self.credentials)
            .await
            .map_err(to_core_error)?;
        // The cached listing still has the old displayname. Drop
        // both calendar + task-list caches conservatively — some
        // servers expose VEVENT + VTODO collections at the same
        // URL, and a rename there would shift the name in both
        // listings.
        self.invalidate_listing_caches();
        Ok(())
    }
}

#[async_trait]
impl TasksFeature for CaldavAdapter {
    async fn list_task_lists(&self) -> CoreResult<Vec<TaskList>> {
        if let Some(cached) = self.cached_task_lists() {
            return Ok(cached);
        }
        let discovery = self.discover().await.map_err(to_core_error)?;
        let fresh = tasks::list_task_lists(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        *self.task_lists_cache.lock().expect("poison") = Some(ListingCache {
            items: fresh.clone(),
            cached_at: chrono::Utc::now(),
        });
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        let list_url = Url::parse(list_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        tasks::get_tasks(&self.http, &list_url, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(
        &self,
        list_id: &str,
        task: NewTask,
    ) -> CoreResult<Task> {
        let list_url = Url::parse(list_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        tasks::create_task(&self.http, &list_url, task, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        tasks::update_task(&self.http, task, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        // Same shape as delete_event: the trait signature loses the
        // list id so we walk the home set and try each candidate
        // collection. 6b.4 will refactor the trait to carry the
        // list/calendar id alongside the row id.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let lists = tasks::list_task_lists(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        for list in lists {
            let url = match Url::parse(&list.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            if let Ok(()) = tasks::delete_task(
                &self.http,
                &url,
                task_id,
                None,
                &self.credentials,
            )
            .await
            {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "task '{task_id}' not found in any list"
        )))
    }

    async fn rename_task_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        // Same as calendars — VTODO collections are renamed via the
        // same PROPPATCH on the collection URL.
        let url = Url::parse(list_id).map_err(|e| {
            CoreError::InvalidInput(format!("task list id is not a URL: {e}"))
        })?;
        calendars::proppatch_displayname(&self.http, &url, new_name, &self.credentials)
            .await
            .map_err(to_core_error)?;
        self.invalidate_listing_caches();
        Ok(())
    }
}

#[async_trait]
impl ContactsFeature for CaldavAdapter {
    async fn list_contact_lists(&self) -> CoreResult<Vec<ContactList>> {
        if let Some(cached) = self.cached_contact_lists() {
            return Ok(cached);
        }
        let discovery = self.discover().await.map_err(to_core_error)?;
        let Some(home) = discovery.addressbook_home_url.as_ref() else {
            // No CardDAV on this server (or the addressbook-home
            // probe failed silently during discovery). Cache an
            // empty Vec so we don't re-walk discovery every time
            // the sidebar refreshes — the registry still routes a
            // future contact through whichever adapter is around.
            *self.contact_lists_cache.lock().expect("poison") = Some(ListingCache {
                items: Vec::new(),
                cached_at: chrono::Utc::now(),
            });
            return Ok(Vec::new());
        };
        let fresh =
            contacts::list_contact_lists(&self.http, home, &self.credentials)
                .await
                .map_err(to_core_error)?;
        *self.contact_lists_cache.lock().expect("poison") = Some(ListingCache {
            items: fresh.clone(),
            cached_at: chrono::Utc::now(),
        });
        Ok(fresh)
    }

    async fn get_contacts(&self, list_id: &str) -> CoreResult<Vec<Contact>> {
        let list_url = Url::parse(list_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        contacts::get_contacts(&self.http, &list_url, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn search_contacts(&self, query: &str) -> CoreResult<Vec<Contact>> {
        // CardDAV defines `addressbook-query` REPORT with `prop-filter`
        // that the server can execute, but implementations are
        // wildly inconsistent (iCloud rejects most non-trivial
        // queries with 403; Nextcloud handles them, Radicale
        // partially). The cheap, universally-correct alternative
        // is to read every book and filter client-side — the
        // listings are cached anyway, and a few hundred vCards is
        // a comfortable in-memory grep. Empty / whitespace queries
        // short-circuit so a stray keystroke doesn't trigger a
        // sync.
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let lists = self.list_contact_lists().await?;
        let mut out = Vec::new();
        for list in lists {
            // Per-list errors don't fail the whole search — same
            // tolerance pattern the calendar fan-out uses.
            let Ok(contacts) = self.get_contacts(&list.id).await else {
                continue;
            };
            for c in contacts {
                if contact_matches(&c, &needle) {
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
        let list_url = Url::parse(list_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        contacts::create_contact(&self.http, &list_url, contact, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn update_contact(&self, contact: Contact) -> CoreResult<Contact> {
        contacts::update_contact(&self.http, contact, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn delete_contact(&self, contact_id: &str) -> CoreResult<()> {
        // Same trait-signature limitation as delete_event /
        // delete_task: only the id, not the parent collection.
        // Walk every known book and try each — the first 2xx wins,
        // 404s keep us moving.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let Some(home) = discovery.addressbook_home_url.as_ref() else {
            return Err(CoreError::NotFound(format!(
                "no addressbook-home-set; contact '{contact_id}' is not routable",
            )));
        };
        let lists =
            contacts::list_contact_lists(&self.http, home, &self.credentials)
                .await
                .map_err(to_core_error)?;
        let mut last_err: Option<CaldavError> = None;
        for list in lists {
            let Ok(url) = Url::parse(&list.id) else {
                continue;
            };
            match contacts::delete_contact(
                &self.http,
                &url,
                contact_id,
                None,
                &self.credentials,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    if matches!(err, CaldavError::Http { status: 404, .. }) {
                        continue;
                    }
                    last_err = Some(err);
                }
            }
        }
        Err(last_err.map(to_core_error).unwrap_or_else(|| {
            CoreError::NotFound(format!(
                "contact '{contact_id}' not found in any address book"
            ))
        }))
    }

    async fn rename_contact_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        // Address book displayname rename is the same PROPPATCH
        // shape as calendars / task lists. iCloud rejects this
        // (read-only address books); other servers (Nextcloud,
        // Radicale) accept it. The override-aware command layer
        // falls back to a local rename on Unsupported, but we
        // surface server-side errors verbatim here so the user
        // sees the real reason.
        let url = Url::parse(list_id).map_err(|e| {
            CoreError::InvalidInput(format!("contact list id is not a URL: {e}"))
        })?;
        calendars::proppatch_displayname(&self.http, &url, new_name, &self.credentials)
            .await
            .map_err(to_core_error)?;
        // Drop the contacts cache so the next listing sees the
        // new name. Calendars and tasks aren't affected.
        *self.contact_lists_cache.lock().expect("poison") = None;
        Ok(())
    }

    async fn get_contact_photo(
        &self,
        contact_id: &str,
    ) -> CoreResult<Option<cal_core::ContactPhoto>> {
        let base = self.contact_resource_base().await?;
        contacts::get_contact_photo(&self.http, &base, contact_id, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn set_contact_photo(
        &self,
        contact_id: &str,
        photo: cal_core::ContactPhoto,
    ) -> CoreResult<()> {
        let base = self.contact_resource_base().await?;
        contacts::set_contact_photo(
            &self.http,
            &base,
            contact_id,
            photo,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)
    }

    async fn delete_contact_photo(&self, contact_id: &str) -> CoreResult<()> {
        let base = self.contact_resource_base().await?;
        contacts::delete_contact_photo(&self.http, &base, contact_id, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn invalidate_contacts_cache(&self) -> CoreResult<()> {
        // CardDAV's listing cache is the only stateful contact
        // cache the adapter holds — individual contact rows are
        // fetched per-request without an intermediate cache.
        *self.contact_lists_cache.lock().expect("poison") = None;
        Ok(())
    }
}

/// Case-insensitive substring match across the same fields the
/// local adapter's `search_contacts` looks at. Kept inline since
/// the predicate is small and self-contained.
fn contact_matches(c: &Contact, needle_lower: &str) -> bool {
    let probes = [
        Some(c.display_name.as_str()),
        c.given_name.as_deref(),
        c.family_name.as_deref(),
        c.organization.as_deref(),
    ];
    if probes
        .iter()
        .flatten()
        .any(|s| s.to_lowercase().contains(needle_lower))
    {
        return true;
    }
    c.emails.iter().any(|e| e.to_lowercase().contains(needle_lower))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthKind, CaldavAccountConfig};
    use mockito::Server;

    const PRINCIPAL_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/alice/</d:href>
        </d:current-user-principal>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    const HOME_SET_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/alice/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/alice/</d:href>
        </c:calendar-home-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn discover_caches_after_first_call() {
        let mut server = Server::new_async().await;
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        // Expect exactly one PROPFIND per phase — second discover()
        // call must hit the cache, not the wire.
        let _principal = server
            .mock("PROPFIND", "/")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .expect(1)
            .create_async()
            .await;
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .with_status(207)
            .with_body(HOME_SET_RESPONSE)
            .expect(1)
            .create_async()
            .await;

        let adapter = CaldavAdapter::new(
            Credentials::new(
                CaldavAccountConfig {
                    server_url: server.url(),
                    username: "alice".into(),
                    auth_kind: AuthKind::Basic,
                },
                "hunter2".into(),
            ),
            Some(Duration::from_secs(3)),
        )
        .unwrap();

        let first = adapter.discover().await.unwrap();
        let second = adapter.discover().await.unwrap();
        assert_eq!(first.calendar_home_url, second.calendar_home_url);
        assert!(adapter.cached_calendar_home().is_some());
    }

    /// Minimal PROPFIND-on-home-set response: one VEVENT-capable
    /// calendar at `/calendars/alice/work/`. Enough for the cache /
    /// invalidation tests below.
    const HOME_LISTING_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Work</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VEVENT"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;

    fn build_adapter(server: &Server) -> CaldavAdapter {
        CaldavAdapter::new(
            Credentials::new(
                CaldavAccountConfig {
                    server_url: server.url(),
                    username: "alice".into(),
                    auth_kind: AuthKind::Basic,
                },
                "hunter2".into(),
            ),
            Some(Duration::from_secs(3)),
        )
        .unwrap()
    }

    async fn mock_discovery_chain(server: &mut Server) {
        server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        server
            .mock("PROPFIND", "/")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .create_async()
            .await;
        server
            .mock("PROPFIND", "/principals/users/alice/")
            .with_status(207)
            .with_body(HOME_SET_RESPONSE)
            .create_async()
            .await;
    }

    #[tokio::test]
    async fn list_calendars_uses_the_listing_cache() {
        let mut server = Server::new_async().await;
        mock_discovery_chain(&mut server).await;
        // Only ONE PROPFIND on the calendar-home is expected — the
        // second `list_calendars` must serve from the cache.
        let m = server
            .mock("PROPFIND", "/calendars/alice/")
            .with_status(207)
            .with_body(HOME_LISTING_RESPONSE)
            .expect(1)
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let first = adapter.list_calendars().await.unwrap();
        let second = adapter.list_calendars().await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first.len(), second.len());
        assert_eq!(first[0].name, "Work");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn invalidate_listing_caches_forces_a_fresh_propfind() {
        let mut server = Server::new_async().await;
        mock_discovery_chain(&mut server).await;
        // Two PROPFINDs expected: one before invalidate, one after.
        let m = server
            .mock("PROPFIND", "/calendars/alice/")
            .with_status(207)
            .with_body(HOME_LISTING_RESPONSE)
            .expect(2)
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        adapter.list_calendars().await.unwrap();
        adapter.invalidate_listing_caches();
        adapter.list_calendars().await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn zero_ttl_bypasses_the_cache_on_every_call() {
        let mut server = Server::new_async().await;
        mock_discovery_chain(&mut server).await;
        // TTL=0 ⇒ every call walks the network.
        let m = server
            .mock("PROPFIND", "/calendars/alice/")
            .with_status(207)
            .with_body(HOME_LISTING_RESPONSE)
            .expect(2)
            .create_async()
            .await;

        let adapter = build_adapter(&server).with_listing_ttl(Duration::ZERO);
        adapter.list_calendars().await.unwrap();
        adapter.list_calendars().await.unwrap();
        m.assert_async().await;
    }
}
