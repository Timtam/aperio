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
pub mod ctag;
pub mod discovery;
pub mod error;
pub mod events;
pub mod freebusy;
mod http;
pub mod mapping;
pub mod sync;
pub mod tasks;
pub mod vcard;
mod vtimezone;
mod xml;

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AttendeeStatus, AuthToken, Calendar, CalendarFeature, Capability, ChangeSet, Contact,
    ContactList, ContactsFeature, ContainerColor, Credentials as CoreCredentials, DateRange,
    Error as CoreError, Event, FreeBusy, NewContact, NewEvent, NewTask, Result as CoreResult, Task,
    TaskList, TasksFeature,
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
    pub fn new(credentials: Credentials, connect_timeout: Option<Duration>) -> CaldavResult<Self> {
        let connect = connect_timeout.unwrap_or(Duration::from_secs(10));
        // Both clients drop idle pooled connections after 25 s — well below
        // reqwest's 90 s default. iCloud's CalDAV hosts kill idle keep-alive
        // sockets much earlier than that, and a request riding such a dead
        // socket fails with "error sending request" before any HTTP response
        // (seen as a spurious network error on the first write after the app
        // sat idle). See `http::SendRetrying` for the second line of defense.
        let pool_idle = Duration::from_secs(25);
        // Production client: follow redirects up to 5 hops. CalDAV
        // PROPFIND / PROPPATCH / REPORT / PUT / DELETE on a moved
        // collection should land on the new URL transparently
        // instead of failing the request.
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(connect)
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(pool_idle)
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
            .pool_idle_timeout(pool_idle)
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
        self.listing_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
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

    // ── sync-collection delta helpers (CACHE-9) ────────────────────────

    /// Obtain a fresh tagged delta token for a collection: a `sync:` token
    /// when the server supports RFC 6578 sync-collection, otherwise a
    /// `ctag:` token, otherwise `None`. Collection-agnostic — shared by
    /// the events, tasks and contacts bootstrap paths.
    async fn bootstrap_token(&self, url: &Url) -> Option<String> {
        match sync::read_sync_token(&self.http, url, &self.credentials).await {
            Ok(Some(st)) => Some(format!("sync:{st}")),
            _ => ctag::read_ctag(&self.http, url, &self.credentials)
                .await
                .ok()
                .flatten()
                .map(|c| format!("ctag:{c}")),
        }
    }

    /// Bootstrap the events delta (no prior token).
    ///
    /// Enumerates every resource via a Depth-1 PROPFIND, then batched
    /// multiget fetches the bodies. This is the only approach that holds
    /// up on large iCloud calendars: a windowed `calendar-query` REPORT
    /// makes iCloud scan the whole collection to apply the time-range
    /// filter and times out (>30s); an empty-token `sync-collection`
    /// answers with only a partial set (so most of the calendar would be
    /// missing). The fresh sync-token comes from `bootstrap_token`, so
    /// every subsequent refresh is a fast per-resource delta.
    ///
    /// Servers that don't answer the PROPFIND fall back to the windowed
    /// time-range read + CTag/legacy token (small/legacy CalDAV servers).
    async fn events_bootstrap(
        &self,
        cal_url: &Url,
        range: DateRange,
    ) -> CoreResult<ChangeSet<Event>> {
        match sync::list_resource_hrefs(&self.http, cal_url, &self.credentials).await {
            Ok(hrefs) => {
                // Folder-complete: the PROPFIND enumerated EVERY resource,
                // so multiget the whole collection (unbounded range) and
                // mark the set complete. The host then caches an unbounded
                // window and serves any later range from the snapshot
                // instead of forcing a fresh cold sync per view.
                let (changes, skipped) = self
                    .multiget_events_windowed(cal_url, &hrefs, whole_collection_range())
                    .await?;
                // Persist the sync-token ONLY when the snapshot is complete. If the
                // server wouldn't serve some resources, leave the token unset: the
                // next refresh re-bootstraps and retries them. Persisting it here
                // would advance past the gap and hide those events for good (deltas
                // only carry future CHANGES, never the resources we skipped).
                let new_token = if skipped == 0 {
                    self.bootstrap_token(cal_url).await
                } else {
                    None
                };
                Ok(ChangeSet {
                    changes,
                    deletions: Vec::new(),
                    new_token,
                    full_resync: true,
                    complete: true,
                })
            }
            Err(err) => {
                // The server didn't answer the resource-enumeration PROPFIND, so we
                // fall back to a windowed time-range read. The cached window then
                // stays BOUNDED to `range` (complete=false), so dates outside it are
                // a cache miss until a later read in that window refreshes — a prime
                // suspect when events "go missing" navigating far ahead or back.
                tracing::warn!(
                    target: "aperio::caldav",
                    calendar = %cal_url,
                    ?err,
                    range_start = %range.start.to_rfc3339(),
                    range_end = %range.end.to_rfc3339(),
                    "CalDAV PROPFIND enumeration failed; falling back to a windowed \
                     range read (bounded cache window, complete=false)"
                );
                let events = events::get_events(&self.http, cal_url, range, &self.credentials)
                    .await
                    .map_err(to_core_error)?;
                let new_token = self.bootstrap_token(cal_url).await;
                Ok(ChangeSet {
                    changes: events,
                    deletions: Vec::new(),
                    new_token,
                    full_resync: true,
                    complete: false,
                })
            }
        }
    }

    /// `calendar-multiget` the bodies of `hrefs` in bounded batches and
    /// map them into in-window events.
    ///
    /// Batching keeps each REPORT well under the request timeout when the
    /// initial sync enumerates a large calendar's resource set. On a batch
    /// failure we retry the batch ONE resource at a time and skip any that
    /// still fail: iCloud can hang indefinitely serving `calendar-data`
    /// for the odd corrupt resource, and a single unreadable event must
    /// not sink the whole calendar. An error is only surfaced when EVERY
    /// resource failed (a real outage) — so a healthy snapshot, minus the
    /// bad resource, is cached and the sync-token persists.
    async fn multiget_events_windowed(
        &self,
        cal_url: &Url,
        hrefs: &[String],
        range: DateRange,
    ) -> CoreResult<(Vec<Event>, usize)> {
        // 50 resources per REPORT: small enough that even a body-heavy
        // batch lands well inside the 30s client timeout, large enough to
        // keep the round-trip count sane on big calendars.
        const MULTIGET_BATCH: usize = 50;
        let mut changes = Vec::new();
        let mut fetched = 0usize;
        // Hrefs the server wouldn't serve even one-at-a-time. Tracked (not just
        // counted) so the warning can name them, and so the caller can refuse to
        // advance the sync-token past a gap — leaving the resources to a later
        // retry instead of masking their events permanently.
        let mut skipped: Vec<String> = Vec::new();
        let mut last_err: Option<CoreError> = None;
        for batch in hrefs.chunks(MULTIGET_BATCH) {
            match self.multiget_batch(cal_url, batch, range).await {
                Ok((mut evs, n)) => {
                    changes.append(&mut evs);
                    fetched += n;
                }
                // Batch failed and there's more than one resource in it —
                // isolate the offender by fetching each on its own.
                Err(_) if batch.len() > 1 => {
                    for href in batch {
                        match self
                            .multiget_batch(cal_url, std::slice::from_ref(href), range)
                            .await
                        {
                            Ok((mut evs, n)) => {
                                changes.append(&mut evs);
                                fetched += n;
                            }
                            Err(e) => {
                                skipped.push(href.clone());
                                last_err = Some(e);
                            }
                        }
                    }
                }
                Err(e) => {
                    skipped.extend(batch.iter().cloned());
                    last_err = Some(e);
                }
            }
        }
        // Nothing came back AND something failed → a real outage, not a
        // single poison resource. Propagate so the caller serves stale and
        // retries, rather than caching an empty snapshot + persisting the
        // token (which would mask the calendar until the token resets).
        if fetched == 0 && !skipped.is_empty() {
            return Err(last_err.expect("a skipped resource implies a recorded error"));
        }
        // Some resources came back but the server wouldn't serve others — their
        // events are NOT in this snapshot. Surface it (the desktop dlopen plugin
        // now forwards its tracing to the host log) and report the count so the
        // caller leaves the sync-token un-advanced.
        if !skipped.is_empty() {
            tracing::warn!(
                target: "aperio::caldav",
                calendar = %cal_url,
                skipped = skipped.len(),
                hrefs = ?skipped,
                "CalDAV multiget could not fetch some resources; their events are \
                 omitted from this snapshot. Leaving the sync-token un-advanced so \
                 the next refresh retries them instead of masking them permanently."
            );
        }
        // Per-calendar fetch summary so a "missing events" report can be traced to
        // the layer that dropped them: `enumerated` = resources the PROPFIND listed,
        // `returned` = resources the server actually served, `events` = rows mapped
        // into the snapshot, `skipped` = resources the server refused. A healthy
        // sync has enumerated == returned + skipped; `events` may legitimately be
        // lower (a VTODO resource maps to no event), but events far below `returned`
        // points at the mapper.
        tracing::info!(
            target: "aperio::caldav",
            calendar = %cal_url,
            enumerated = hrefs.len(),
            returned = fetched,
            events = changes.len(),
            skipped = skipped.len(),
            "CalDAV multiget complete",
        );
        Ok((changes, skipped.len()))
    }

    /// One `calendar-multiget` REPORT for `hrefs`. Returns the in-window
    /// events plus the count of resources the server actually returned —
    /// the count lets the caller tell "fetched but filtered out of the
    /// window" apart from "the fetch itself failed".
    async fn multiget_batch(
        &self,
        cal_url: &Url,
        hrefs: &[String],
        range: DateRange,
    ) -> CoreResult<(Vec<Event>, usize)> {
        let calendar_id = cal_url.as_str();
        let entries = sync::calendar_multiget(&self.http, cal_url, hrefs, &self.credentials)
            .await
            .map_err(to_core_error)?;
        let fetched = entries.len();
        let mut out = Vec::new();
        for entry in entries {
            let Some(ical) = entry.calendar_data else {
                continue;
            };
            let Ok(mut evs) =
                mapping::parse_calendar_data_with_href(&ical, calendar_id, Some(&entry.href))
            else {
                continue;
            };
            for ev in &mut evs {
                if let Some(etag) = entry.etag.clone() {
                    ev.etag = Some(etag);
                }
                // Singles outside the cached window are dropped; recurring
                // masters always pass (the frontend expands them).
                if event_in_window(ev, range) {
                    out.push(ev.clone());
                }
            }
        }
        Ok((out, fetched))
    }

    /// Bootstrap the tasks delta: a full VTODO read plus a fresh token.
    async fn tasks_bootstrap(&self, list_url: &Url) -> CoreResult<ChangeSet<Task>> {
        let tasks = tasks::get_tasks(&self.http, list_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        let new_token = self.bootstrap_token(list_url).await;
        Ok(ChangeSet {
            changes: tasks,
            deletions: Vec::new(),
            new_token,
            full_resync: true,
            complete: false,
        })
    }

    /// CTag-gated tasks path for servers without sync-collection.
    async fn tasks_ctag_gated(
        &self,
        list_url: &Url,
        prev_ctag: Option<&str>,
    ) -> CoreResult<ChangeSet<Task>> {
        let ctag = ctag::read_ctag(&self.http, list_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        if let (Some(current), Some(prev)) = (ctag.as_deref(), prev_ctag) {
            if current == prev {
                return Ok(ChangeSet {
                    changes: Vec::new(),
                    deletions: Vec::new(),
                    new_token: Some(format!("ctag:{current}")),
                    full_resync: false,
                    complete: false,
                });
            }
        }
        let tasks = tasks::get_tasks(&self.http, list_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes: tasks,
            deletions: Vec::new(),
            new_token: ctag.map(|c| format!("ctag:{c}")),
            full_resync: true,
            complete: false,
        })
    }

    /// Per-resource sync-collection tasks delta. Unlike events, a CalDAV
    /// task's cal-core id is `{href}|{uid}`, so a removed href maps to the
    /// cached row's `native_id` directly — deletions need no full re-list.
    async fn tasks_sync_incremental(
        &self,
        list_url: &Url,
        sync_token: &str,
    ) -> CoreResult<ChangeSet<Task>> {
        let result = match sync::sync_collection(
            &self.http,
            list_url,
            sync_token,
            &self.credentials,
        )
        .await
        {
            Ok(r) => r,
            Err(_) => return self.tasks_bootstrap(list_url).await,
        };
        let next_token = result
            .sync_token
            .clone()
            .unwrap_or_else(|| sync_token.to_string());
        let entries =
            sync::calendar_multiget(&self.http, list_url, &result.changed, &self.credentials)
                .await
                .map_err(to_core_error)?;
        let mut changes = tasks::parse_task_entries(&entries, list_url.as_str());
        // A changed subtask's RELATED-TO may name a parent that didn't
        // itself change — its `{href}|{uid}` id then isn't derivable from
        // the delta alone. One id listing supplies the missing uid → id
        // entries (rare: only deltas whose parent link crosses the change
        // set pay for it; the listing parses tolerantly, so one garbled
        // resource can't sink it). When even that read fails, FAIL the
        // whole delta: the token isn't advanced and the next sync retries,
        // instead of persisting a falsified flat parent into the cache —
        // which a later Aperio edit would then write back to the server.
        let mut index = tasks::uid_index(&changes);
        if tasks::any_unresolved_parent(&changes, &index) {
            let full = tasks::get_task_uid_index(&self.http, list_url, &self.credentials)
                .await
                .map_err(to_core_error)?;
            index.extend(full);
        }
        tasks::resolve_parent_ids(&mut changes, &index);
        Ok(ChangeSet {
            changes,
            deletions: result.deleted,
            new_token: Some(format!("sync:{next_token}")),
            full_resync: false,
            complete: false,
        })
    }

    /// Bootstrap the contacts delta: a full vCard read plus a fresh token.
    async fn contacts_bootstrap(&self, book_url: &Url) -> CoreResult<ChangeSet<Contact>> {
        let contacts = contacts::get_contacts(&self.http, book_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        let new_token = self.bootstrap_token(book_url).await;
        Ok(ChangeSet {
            changes: contacts,
            deletions: Vec::new(),
            new_token,
            full_resync: true,
            complete: false,
        })
    }

    /// CTag-gated contacts path for servers without sync-collection.
    async fn contacts_ctag_gated(
        &self,
        book_url: &Url,
        prev_ctag: Option<&str>,
    ) -> CoreResult<ChangeSet<Contact>> {
        let ctag = ctag::read_ctag(&self.http, book_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        if let (Some(current), Some(prev)) = (ctag.as_deref(), prev_ctag) {
            if current == prev {
                return Ok(ChangeSet {
                    changes: Vec::new(),
                    deletions: Vec::new(),
                    new_token: Some(format!("ctag:{current}")),
                    full_resync: false,
                    complete: false,
                });
            }
        }
        let contacts = contacts::get_contacts(&self.http, book_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes: contacts,
            deletions: Vec::new(),
            new_token: ctag.map(|c| format!("ctag:{c}")),
            full_resync: true,
            complete: false,
        })
    }

    /// Per-resource sync-collection contacts delta. Like tasks, a CalDAV
    /// contact's id is `{href}|{uid}`, so removed hrefs map to `native_id`
    /// and per-resource deletions work directly.
    async fn contacts_sync_incremental(
        &self,
        book_url: &Url,
        sync_token: &str,
    ) -> CoreResult<ChangeSet<Contact>> {
        let result = match sync::sync_collection(
            &self.http,
            book_url,
            sync_token,
            &self.credentials,
        )
        .await
        {
            Ok(r) => r,
            Err(_) => return self.contacts_bootstrap(book_url).await,
        };
        let next_token = result
            .sync_token
            .clone()
            .unwrap_or_else(|| sync_token.to_string());
        let entries =
            sync::addressbook_multiget(&self.http, book_url, &result.changed, &self.credentials)
                .await
                .map_err(to_core_error)?;
        let changes = contacts::parse_contact_entries(&entries, book_url.as_str());
        Ok(ChangeSet {
            changes,
            deletions: result.deleted,
            new_token: Some(format!("sync:{next_token}")),
            full_resync: false,
            complete: false,
        })
    }

    /// CTag-gated path for servers without sync-collection. Unchanged ⇒
    /// empty no-op; changed ⇒ full windowed re-list.
    async fn events_ctag_gated(
        &self,
        cal_url: &Url,
        range: DateRange,
        prev_ctag: Option<&str>,
    ) -> CoreResult<ChangeSet<Event>> {
        let ctag = ctag::read_ctag(&self.http, cal_url, &self.credentials)
            .await
            .map_err(to_core_error)?;
        if let (Some(current), Some(prev)) = (ctag.as_deref(), prev_ctag) {
            if current == prev {
                return Ok(ChangeSet {
                    changes: Vec::new(),
                    deletions: Vec::new(),
                    new_token: Some(format!("ctag:{current}")),
                    full_resync: false,
                    complete: false,
                });
            }
        }
        let events = events::get_events(&self.http, cal_url, range, &self.credentials)
            .await
            .map_err(to_core_error)?;
        Ok(ChangeSet {
            changes: events,
            deletions: Vec::new(),
            new_token: ctag.map(|c| format!("ctag:{c}")),
            full_resync: true,
            complete: false,
        })
    }

    /// Per-resource sync-collection events path. Multigets only the
    /// changed resources (range-filtering singles) and emits removed
    /// hrefs as per-resource deletions — the `{href}|{uid}` event id
    /// means a removed href maps onto the cache row's `native_id`
    /// directly. Any sync-collection failure (expired token, server
    /// hiccup) recovers via a clean bootstrap.
    async fn events_sync_incremental(
        &self,
        cal_url: &Url,
        range: DateRange,
        sync_token: &str,
    ) -> CoreResult<ChangeSet<Event>> {
        let result =
            match sync::sync_collection(&self.http, cal_url, sync_token, &self.credentials).await {
                Ok(r) => r,
                Err(_) => return self.events_bootstrap(cal_url, range).await,
            };
        let next_token = result
            .sync_token
            .clone()
            .unwrap_or_else(|| sync_token.to_string());

        // Folder-complete: the snapshot already holds the whole collection
        // (the bootstrap cached it unbounded), so fold the changed
        // resources in at any date — no view-window filter — and keep the
        // set marked complete.
        let (changes, skipped) = self
            .multiget_events_windowed(cal_url, &result.changed, whole_collection_range())
            .await?;
        // If a changed resource couldn't be fetched, keep the OLD sync-token so the
        // next delta re-runs from here and retries it — advancing past it would drop
        // that change permanently (the next delta would never report it again).
        let token: &str = if skipped == 0 {
            &next_token
        } else {
            sync_token
        };
        tracing::info!(
            target: "aperio::caldav",
            calendar = %cal_url,
            changed = result.changed.len(),
            deleted = result.deleted.len(),
            "CalDAV incremental delta applied",
        );
        Ok(ChangeSet {
            changes,
            deletions: result.deleted,
            new_token: Some(format!("sync:{token}")),
            full_resync: false,
            complete: true,
        })
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
                "no addressbook-home-set; this server does not advertise CardDAV".into(),
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

    /// The user's `mailto:` organizer address for a write — but only when the
    /// caller opted to notify AND the server actually auto-schedules (RFC
    /// 6638). Otherwise `None`, so the mapper omits `ORGANIZER`/`ATTENDEE`
    /// and the server schedules nothing. Discovery is cached, so this is free
    /// after the first call; a discovery failure degrades to `None` (store
    /// the event silently rather than failing the write).
    async fn organizer_for_send(&self, sending: bool) -> Option<String> {
        if !sending {
            return None;
        }
        self.discover()
            .await
            .ok()
            .filter(|d| d.supports_scheduling)
            .and_then(|d| d.calendar_user_address)
    }

    /// Whether this account stores a per-event color natively (RFC 7986
    /// `COLOR`). True for every CalDAV server *except* iCloud: iCloud
    /// auto-schedules (RFC 6638), so a `COLOR`-bearing PUT on an event with
    /// attendees would email them — Stage 1 keeps iCloud per-event colors as
    /// host-local overrides instead. URL-based heuristic; a generic server
    /// that silently ignores `COLOR` simply drops the color on next sync
    /// (rare, user-reportable). Drives both the calendar capability flag and
    /// the write-side gate that clears `color_hex` before an iCloud PUT.
    fn supports_event_color(&self) -> bool {
        !self.credentials.config.server_url.contains("icloud.com")
    }

    /// Test-only: peek at the cached result without going to the wire.
    #[cfg(test)]
    fn cached_calendar_home(&self) -> Option<url::Url> {
        self.discovery
            .lock()
            .expect("poison")
            .as_ref()
            .and_then(|d| d.calendar_home_url.clone())
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

/// Window predicate for the sync-collection incremental read: recurring
/// masters always pass (the frontend expander handles the visible
/// window); singles must overlap `[range.start, range.end)`. Mirrors the
/// EWS/Google/Graph delta adapters so the cache stays windowed even
/// though `sync-collection` reports across the whole collection.
fn event_in_window(ev: &Event, range: DateRange) -> bool {
    ev.recurrence.is_some() || (ev.end >= range.start && ev.start < range.end)
}

/// The unbounded range used by the folder-complete event sync. A CalDAV
/// calendar reached via PROPFIND-enumeration + RFC 6578 sync-collection is
/// cached in its entirety (every resource, any date), so the multiget must
/// keep events of all dates instead of filtering to a view window. Passing
/// this range makes [`event_in_window`] accept everything; the change set
/// is then marked `complete: true` and the host records an unbounded
/// snapshot window, so every later view range is served straight from the
/// cache (no per-range cold re-sync).
fn whole_collection_range() -> DateRange {
    DateRange::new(
        chrono::DateTime::<chrono::Utc>::MIN_UTC,
        chrono::DateTime::<chrono::Utc>::MAX_UTC,
    )
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
        // CardDAV-only server (no calendar-home-set) → no calendars.
        let Some(home) = discovery.calendar_home_url.as_ref() else {
            return Ok(Vec::new());
        };
        let mut fresh = calendars::list_calendars(
            &self.http,
            home,
            &self.credentials,
            discovery.supports_scheduling,
        )
        .await
        .map_err(to_core_error)?;
        // RFC 7986 native per-event COLOR: advertise it for every calendar on
        // a color-capable (non-iCloud) account so the host routes recolors
        // through the provider; iCloud keeps the Stage 1 host-local override.
        let color_capable = self.supports_event_color();
        for cal in &mut fresh {
            cal.supports_event_color = color_capable;
        }
        *self.calendars_cache.lock().expect("poison") = Some(ListingCache {
            items: fresh.clone(),
            cached_at: chrono::Utc::now(),
        });
        Ok(fresh)
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> CoreResult<Vec<Event>> {
        // The calendar id is the absolute collection URL produced
        // by `list_calendars`. Re-parse it so the request lands at
        // the exact path the server told us about; falling back to
        // a join against the discovered home would be too lax.
        let cal_url =
            Url::parse(calendar_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        events::get_events(&self.http, &cal_url, range, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn get_events_delta(
        &self,
        calendar_id: &str,
        range: DateRange,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Event>> {
        let cal_url =
            Url::parse(calendar_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        // The token is tagged so we know which mechanism minted it:
        //   `sync:<token>` → RFC 6578 per-resource sync-collection,
        //   `ctag:<ctag>`  → the CTag gate (server lacks sync-collection),
        //   None / bare    → bootstrap (legacy CTag tokens land here too,
        //                     and upgrade to `sync:` if the server supports it).
        match since_token {
            Some(t) => {
                if let Some(sync_token) = t.strip_prefix("sync:") {
                    self.events_sync_incremental(&cal_url, range, sync_token)
                        .await
                } else if let Some(ctag) = t.strip_prefix("ctag:") {
                    self.events_ctag_gated(&cal_url, range, Some(ctag)).await
                } else {
                    self.events_bootstrap(&cal_url, range).await
                }
            }
            None => self.events_bootstrap(&cal_url, range).await,
        }
    }

    async fn create_event(&self, calendar_id: &str, mut event: NewEvent) -> CoreResult<Event> {
        let cal_url =
            Url::parse(calendar_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        // Defense-in-depth iCloud gate: the host already withholds `color_hex`
        // from non-capable calendars, but a stray value (e.g. a cross-calendar
        // move into iCloud) must never reach a COLOR-bearing PUT here.
        if !self.supports_event_color() {
            event.color_hex = None;
        }
        let organizer = self.organizer_for_send(event.send_invitations).await;
        events::create_event(
            &self.http,
            &cal_url,
            event,
            &self.credentials,
            organizer.as_deref(),
        )
        .await
        .map_err(to_core_error)
    }

    async fn update_event(&self, mut event: Event) -> CoreResult<Event> {
        if !self.supports_event_color() {
            event.color_hex = None;
        }
        let organizer = self.organizer_for_send(event.send_invitations).await;
        events::update_event(&self.http, event, &self.credentials, organizer.as_deref())
            .await
            .map_err(to_core_error)
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: chrono::DateTime<chrono::Utc>,
        _send_cancellations: bool,
    ) -> CoreResult<()> {
        // `send_cancellations` is not honoured explicitly: CalDAV scheduling is
        // SERVER-driven (RFC 6638 implicit scheduling). On a scheduling-aware
        // collection (e.g. iCloud) adding the EXDATE to the organizer's event
        // makes the server itself email a per-occurrence CANCEL to attendees;
        // non-scheduling servers just store the EXDATE. Either way the write is
        // the same, so we ignore the flag here.
        //
        // Same walk-the-home-set workaround as delete_event below —
        // the trait signature loses the parent calendar id, so we
        // try every calendar in the home set until one accepts the
        // EXDATE update. The aperio command layer routes via the
        // registry's calendar→account map, so production hits this
        // path with the right adapter already; the walk is the
        // fallback if a caller forgot to thread the calendar_id
        // through.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let cals = match discovery.calendar_home_url.as_ref() {
            Some(home) => calendars::list_calendars(
                &self.http,
                home,
                &self.credentials,
                discovery.supports_scheduling,
            )
            .await
            .map_err(to_core_error)?,
            // No calendar home (CardDAV-only server) → nothing to walk;
            // the loop below falls through to the not-found error.
            None => Vec::new(),
        };
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
            CoreError::NotFound(format!("event '{event_id}' not found in any calendar"))
        }))
    }

    async fn delete_event(&self, event_id: &str, _send_cancellations: bool) -> CoreResult<()> {
        // `send_cancellations` is not honoured here: CalDAV scheduling is
        // SERVER-driven (RFC 6638 implicit scheduling). On a scheduling-aware
        // collection (e.g. iCloud) the server itself emails a CANCEL to every
        // attendee when the ORGANIZER's resource is DELETEd — Aperio can't
        // reliably force that on or off from the client, so a plain DELETE is
        // the honest behaviour either way (notify-on-delete, server's call).
        // The precise choice is offered only where the adapter genuinely
        // controls it (EWS / Graph / Google).
        //
        // The trait signature only gives us the event id. CalDAV
        // needs the calendar collection URL too — we recover it
        // by re-reading the discovery cache; callers that know
        // the calendar URL up front can hit `events::delete_event`
        // directly.
        //
        // Without the calendar id we fall back to a best-effort:
        // walk every calendar in the home set and try the DELETE
        // against each. The walker keys off the typed
        // `DeleteOutcome` rather than a plain Ok/Err — a 404 from
        // the wrong calendar must NOT short-circuit the search.
        // We've been bitten by exactly that before the typed
        // outcome existed: the walker stopped on the first 404 and
        // left the resource untouched in whichever calendar
        // actually owned it.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let cals = match discovery.calendar_home_url.as_ref() {
            Some(home) => calendars::list_calendars(
                &self.http,
                home,
                &self.credentials,
                discovery.supports_scheduling,
            )
            .await
            .map_err(to_core_error)?,
            // No calendar home (CardDAV-only server) → nothing to walk;
            // the loop below falls through to the not-found error.
            None => Vec::new(),
        };
        let mut last_err: Option<CoreError> = None;
        for cal in cals {
            let cal_url = match Url::parse(&cal.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            // Without an ETag we don't bother with If-Match — the
            // user explicitly chose to delete this row, so a
            // concurrent modification is informational at best.
            match events::delete_event(&self.http, &cal_url, event_id, None, &self.credentials)
                .await
            {
                Ok(events::DeleteOutcome::Deleted) => return Ok(()),
                Ok(events::DeleteOutcome::NotFound) => continue,
                Err(err) => {
                    // Non-404 errors might be transient (auth
                    // hiccup, server hiccup). Remember the last
                    // one in case nothing else works, but keep
                    // walking — the resource might still live in
                    // another calendar we haven't tried yet.
                    last_err = Some(to_core_error(err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            CoreError::NotFound(format!("event '{event_id}' not found in any calendar"))
        }))
    }

    async fn get_free_busy(&self, emails: &[&str], range: DateRange) -> CoreResult<Vec<FreeBusy>> {
        if emails.is_empty() {
            return Ok(Vec::new());
        }
        // Free-busy rides RFC 6638: POST an iTIP VFREEBUSY to the
        // principal's schedule-outbox. Needs both the outbox URL and a
        // usable ORGANIZER address (the user's calendar-user-address).
        // Discovery is cached; a server without an outbox — or a
        // discovery failure — degrades to "availability unknown"
        // (empty slots per address) rather than erroring.
        let discovery = match self.discover().await {
            Ok(d) => d,
            Err(_) => return Ok(freebusy::unknown(emails)),
        };
        let (Some(outbox), Some(organizer)) = (
            discovery.schedule_outbox_url,
            discovery.calendar_user_address,
        ) else {
            return Ok(freebusy::unknown(emails));
        };
        freebusy::query_free_busy(
            &self.http,
            &outbox,
            &organizer,
            emails,
            range,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)
    }

    async fn current_user_email(&self) -> CoreResult<Option<String>> {
        // The principal's calendar-user-address (RFC 6638, discovered
        // once and cached) is the user's mailto: identity. Strip the
        // scheme so it matches attendee SMTP addresses. A discovery
        // failure degrades to None (RSVP simply hidden), never an error.
        let discovery = match self.discover().await {
            Ok(d) => d,
            Err(_) => return Ok(None),
        };
        Ok(discovery.calendar_user_address.map(|addr| {
            let a = addr.trim();
            a.strip_prefix("mailto:")
                .or_else(|| a.strip_prefix("MAILTO:"))
                .unwrap_or(a)
                .to_string()
        }))
    }

    async fn respond_to_event(
        &self,
        event_id: &str,
        status: AttendeeStatus,
        send_response: bool,
    ) -> CoreResult<()> {
        // Need our own calendar-user-address to find our ATTENDEE row,
        // and any base URL (scheme+host) — the event id's encoded href
        // supplies the resource path. RFC 6638 servers (iCloud) emit the
        // iTIP REPLY automatically when we PUT the PARTSTAT change.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let self_email = discovery
            .calendar_user_address
            .as_deref()
            .map(|a| {
                let a = a.trim();
                a.strip_prefix("mailto:")
                    .or_else(|| a.strip_prefix("MAILTO:"))
                    .unwrap_or(a)
            })
            .ok_or_else(|| {
                CoreError::Unsupported(
                    "RSVP needs the account's calendar-user-address, which this server didn't advertise".into(),
                )
            })?;
        events::respond_to_event(
            &self.http,
            &discovery.dav_root,
            event_id,
            self_email,
            status,
            send_response,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // The color is fetched together with the listing and lives
        // on the Calendar struct already. A second per-id round-trip
        // would just duplicate work; consumers should use the value
        // off the Calendar they got from `list_calendars`.
        None
    }

    async fn rename_calendar(&self, calendar_id: &str, new_name: &str) -> CoreResult<()> {
        // Calendar id == collection URL in CalDAV (see `to_calendar`).
        let url = Url::parse(calendar_id)
            .map_err(|e| CoreError::InvalidInput(format!("calendar id is not a URL: {e}")))?;
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
        // CardDAV-only server (no calendar-home-set) → no task lists.
        let Some(home) = discovery.calendar_home_url.as_ref() else {
            return Ok(Vec::new());
        };
        let fresh = tasks::list_task_lists(&self.http, home, &self.credentials)
            .await
            .map_err(to_core_error)?;
        *self.task_lists_cache.lock().expect("poison") = Some(ListingCache {
            items: fresh.clone(),
            cached_at: chrono::Utc::now(),
        });
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        let list_url =
            Url::parse(list_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        tasks::get_tasks(&self.http, &list_url, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn get_tasks_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Task>> {
        let list_url =
            Url::parse(list_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        match since_token {
            Some(t) => {
                if let Some(sync_token) = t.strip_prefix("sync:") {
                    self.tasks_sync_incremental(&list_url, sync_token).await
                } else if let Some(ctag) = t.strip_prefix("ctag:") {
                    self.tasks_ctag_gated(&list_url, Some(ctag)).await
                } else {
                    self.tasks_bootstrap(&list_url).await
                }
            }
            None => self.tasks_bootstrap(&list_url).await,
        }
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> CoreResult<Task> {
        let list_url =
            Url::parse(list_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
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
        // Same shape — and same 404-must-not-short-circuit pitfall
        // — as `delete_event`. The walker keys off
        // `DeleteOutcome::Deleted` so a 404 from the wrong task
        // list keeps the search going instead of fooling us into
        // thinking the row was already gone.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let lists = match discovery.calendar_home_url.as_ref() {
            Some(home) => tasks::list_task_lists(&self.http, home, &self.credentials)
                .await
                .map_err(to_core_error)?,
            // No calendar home (CardDAV-only) → nothing to walk.
            None => Vec::new(),
        };
        let mut last_err: Option<CoreError> = None;
        for list in lists {
            let url = match Url::parse(&list.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            match tasks::delete_task(&self.http, &url, task_id, None, &self.credentials).await {
                Ok(events::DeleteOutcome::Deleted) => return Ok(()),
                Ok(events::DeleteOutcome::NotFound) => continue,
                Err(err) => {
                    last_err = Some(to_core_error(err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            CoreError::NotFound(format!("task '{task_id}' not found in any list"))
        }))
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        // Same as calendars — VTODO collections are renamed via the
        // same PROPPATCH on the collection URL.
        let url = Url::parse(list_id)
            .map_err(|e| CoreError::InvalidInput(format!("task list id is not a URL: {e}")))?;
        calendars::proppatch_displayname(&self.http, &url, new_name, &self.credentials)
            .await
            .map_err(to_core_error)?;
        self.invalidate_listing_caches();
        Ok(())
    }

    async fn create_task_list(&self, name: &str, _parent_id: Option<&str>) -> CoreResult<TaskList> {
        let discovery = self.discover().await.map_err(to_core_error)?;
        // Creating a task list needs a calendar home to create the VTODO
        // collection in; a CardDAV-only account has none.
        let Some(home) = discovery.calendar_home_url.as_ref() else {
            return Err(CoreError::Unsupported(
                "this CalDAV account exposes no calendar home; cannot create a task list".into(),
            ));
        };
        let created = tasks::create_task_list(&self.http, home, name, &self.credentials)
            .await
            .map_err(to_core_error)?;
        self.invalidate_listing_caches();
        Ok(created)
    }

    async fn delete_task_list(&self, list_id: &str) -> CoreResult<()> {
        let url = Url::parse(list_id)
            .map_err(|e| CoreError::InvalidInput(format!("task list id is not a URL: {e}")))?;
        tasks::delete_task_list(&self.http, &url, &self.credentials)
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
        let fresh = contacts::list_contact_lists(&self.http, home, &self.credentials)
            .await
            .map_err(to_core_error)?;
        *self.contact_lists_cache.lock().expect("poison") = Some(ListingCache {
            items: fresh.clone(),
            cached_at: chrono::Utc::now(),
        });
        Ok(fresh)
    }

    async fn get_contacts(&self, list_id: &str) -> CoreResult<Vec<Contact>> {
        let list_url =
            Url::parse(list_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        contacts::get_contacts(&self.http, &list_url, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn get_contacts_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Contact>> {
        let list_url =
            Url::parse(list_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        match since_token {
            Some(t) => {
                if let Some(sync_token) = t.strip_prefix("sync:") {
                    self.contacts_sync_incremental(&list_url, sync_token).await
                } else if let Some(ctag) = t.strip_prefix("ctag:") {
                    self.contacts_ctag_gated(&list_url, Some(ctag)).await
                } else {
                    self.contacts_bootstrap(&list_url).await
                }
            }
            None => self.contacts_bootstrap(&list_url).await,
        }
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

    async fn create_contact(&self, list_id: &str, contact: NewContact) -> CoreResult<Contact> {
        let list_url =
            Url::parse(list_id).map_err(|err| CoreError::InvalidInput(err.to_string()))?;
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
        let lists = contacts::list_contact_lists(&self.http, home, &self.credentials)
            .await
            .map_err(to_core_error)?;
        let mut last_err: Option<CaldavError> = None;
        for list in lists {
            let Ok(url) = Url::parse(&list.id) else {
                continue;
            };
            match contacts::delete_contact(&self.http, &url, contact_id, None, &self.credentials)
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

    async fn rename_contact_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        // Address book displayname rename is the same PROPPATCH
        // shape as calendars / task lists. iCloud rejects this
        // (read-only address books); other servers (Nextcloud,
        // Radicale) accept it. The override-aware command layer
        // falls back to a local rename on Unsupported, but we
        // surface server-side errors verbatim here so the user
        // sees the real reason.
        let url = Url::parse(list_id)
            .map_err(|e| CoreError::InvalidInput(format!("contact list id is not a URL: {e}")))?;
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
        contacts::set_contact_photo(&self.http, &base, contact_id, photo, &self.credentials)
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
    c.emails
        .iter()
        .any(|e| e.to_lowercase().contains(needle_lower))
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

    #[test]
    fn supports_event_color_is_false_only_for_icloud() {
        let mk = |url: &str| {
            CaldavAdapter::new(
                Credentials::new(
                    CaldavAccountConfig {
                        server_url: url.into(),
                        username: "alice".into(),
                        auth_kind: AuthKind::Basic,
                    },
                    "hunter2".into(),
                ),
                Some(Duration::from_secs(3)),
            )
            .unwrap()
        };
        // iCloud auto-schedules — a COLOR-bearing PUT would email attendees,
        // so it keeps the host-local override (Stage 1).
        assert!(!mk("https://caldav.icloud.com/123/calendars/").supports_event_color());
        // Generic CalDAV servers round-trip RFC 7986 COLOR natively.
        assert!(mk("https://cloud.example.com/remote.php/dav").supports_event_color());
        assert!(mk("https://radicale.example.org/").supports_event_color());
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

    // A CTag-only server's PROPFIND body: the collection carries the
    // CTag (for the gate + token), and a member resource carries a
    // getetag (for the Depth-1 bootstrap enumeration). The collection's
    // own trailing-slash href is filtered out of the resource list.
    const CTAG_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat><d:prop><cs:getctag>v1</cs:getctag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/e1.ics</d:href>
    <d:propstat><d:prop><d:getetag>"e1"</d:getetag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#;

    const DELTA_REPORT_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/work/e1.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"e1"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:evt-1
SUMMARY:Standup
DTSTART:20260520T080000Z
DTEND:20260520T083000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
    </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#;

    /// PROPFIND `DAV:sync-token` response advertising a token — marks the
    /// server as sync-collection capable.
    use chrono::TimeZone;

    fn delta_range() -> DateRange {
        DateRange::new(
            chrono::Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap(),
        )
    }

    #[tokio::test]
    async fn get_events_delta_falls_back_to_ctag_gate() {
        // No sync-token advertised (PROPFIND returns the CTag body, no
        // <d:sync-token>) → the adapter degrades to the CTag gate and
        // tags the token `ctag:`.
        let mut server = Server::new_async().await;
        let _ctag_mock = server
            .mock("PROPFIND", "/calendars/alice/work/")
            .with_status(207)
            .with_body(CTAG_RESPONSE)
            .create_async()
            .await;
        // Bootstrap enumerates via the PROPFIND above, then makes exactly
        // one multiget REPORT for the enumerated resource. The gated second
        // call (unchanged CTag) skips the REPORT entirely.
        let report_mock = server
            .mock("REPORT", "/calendars/alice/work/")
            .with_status(207)
            .with_body(DELTA_REPORT_RESPONSE)
            .expect(1)
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let cal_url = format!("{}/calendars/alice/work/", server.url());

        let first = adapter
            .get_events_delta(&cal_url, delta_range(), None)
            .await
            .unwrap();
        assert!(first.full_resync);
        assert_eq!(first.changes.len(), 1);
        assert_eq!(first.new_token.as_deref(), Some("ctag:v1"));

        // Tagged ctag token, unchanged CTag → empty no-op, no REPORT.
        let second = adapter
            .get_events_delta(&cal_url, delta_range(), Some("ctag:v1"))
            .await
            .unwrap();
        assert!(!second.full_resync);
        assert!(second.changes.is_empty());
        assert_eq!(second.new_token.as_deref(), Some("ctag:v1"));

        report_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_events_delta_bootstrap_enumerates_via_propfind() {
        // Bootstrap enumerates resources with a Depth-1 PROPFIND (NOT a
        // windowed calendar-query, which times out on large iCloud
        // calendars), batch-multigets the bodies, and tags the fresh
        // sync-token. The collection's own trailing-slash href is dropped
        // from the resource list — multi-getting it would ask the server
        // for the whole calendar and hang.
        let mut server = Server::new_async().await;
        // PROPFIND #1 (getetag): resource enumeration → the collection
        // self-ref (filtered) + one member resource.
        let _enumerate = server
            .mock("PROPFIND", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("getetag".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat><d:prop></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/e1.ics</d:href>
    <d:propstat><d:prop><d:getetag>"e1"</d:getetag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#,
            )
            .create_async()
            .await;
        // Batched multiget fetches the enumerated body.
        let _multiget = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("calendar-multiget".into()))
            .with_status(207)
            .with_body(DELTA_REPORT_RESPONSE)
            .create_async()
            .await;
        // PROPFIND #2 (sync-token): the server advertises a token → tagged
        // `sync:`.
        let _token = server
            .mock("PROPFIND", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("sync-token".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat><d:prop><d:sync-token>ST1</d:sync-token></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#,
            )
            .create_async()
            .await;
        // No time-range calendar-query is allowed on the bootstrap path.
        let no_query = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("calendar-query".into()))
            .with_status(500)
            .expect(0)
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let cal_url = format!("{}/calendars/alice/work/", server.url());
        let first = adapter
            .get_events_delta(&cal_url, delta_range(), None)
            .await
            .unwrap();
        assert!(first.full_resync);
        // Folder-complete: the PROPFIND enumerated the whole collection, so
        // the change set covers any date and the host caches an unbounded
        // window (no per-range cold re-sync on later views).
        assert!(first.complete);
        assert_eq!(first.changes.len(), 1);
        assert_eq!(first.new_token.as_deref(), Some("sync:ST1"));
        no_query.assert_async().await;
    }

    #[tokio::test]
    async fn get_events_delta_sync_incremental_multigets_only_changed() {
        let mut server = Server::new_async().await;
        // sync-collection REPORT → one changed href, fresh token, no deletes.
        let _sync = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("sync-collection".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/e1.ics</d:href>
    <d:propstat><d:prop><d:getetag>"v2"</d:getetag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:sync-token>ST2</d:sync-token>
</d:multistatus>"#,
            )
            .create_async()
            .await;
        // multiget REPORT → the changed resource's body.
        let _multiget = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("calendar-multiget".into()))
            .with_status(207)
            .with_body(DELTA_REPORT_RESPONSE)
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let cal_url = format!("{}/calendars/alice/work/", server.url());
        let cs = adapter
            .get_events_delta(&cal_url, delta_range(), Some("sync:ST1"))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        // Incremental deltas stay folder-complete: the snapshot already
        // holds the whole collection, so changed resources fold in at any
        // date and the window stays unbounded.
        assert!(cs.complete);
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.new_token.as_deref(), Some("sync:ST2"));
    }

    #[tokio::test]
    async fn get_events_delta_sync_emits_per_resource_changes_and_deletions() {
        // With `{href}|{uid}` event ids, a removed href maps to the cache
        // native_id — so a deletion is a per-resource removal, not a full
        // re-list.
        let mut server = Server::new_async().await;
        let _sync = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("sync-collection".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/e1.ics</d:href>
    <d:propstat><d:prop><d:getetag>"v2"</d:getetag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/gone.ics</d:href>
    <d:status>HTTP/1.1 404 Not Found</d:status>
  </d:response>
  <d:sync-token>ST3</d:sync-token>
</d:multistatus>"#,
            )
            .create_async()
            .await;
        let _multiget = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex("calendar-multiget".into()))
            .with_status(207)
            .with_body(DELTA_REPORT_RESPONSE)
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let cal_url = format!("{}/calendars/alice/work/", server.url());
        let cs = adapter
            .get_events_delta(&cal_url, delta_range(), Some("sync:ST1"))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        // The changed event id now carries the href (native_id = href).
        assert!(cs.changes[0].id.ends_with("|evt-1"));
        assert_eq!(
            cs.deletions,
            vec!["/calendars/alice/work/gone.ics".to_string()]
        );
        assert_eq!(cs.new_token.as_deref(), Some("sync:ST3"));
    }

    #[tokio::test]
    async fn get_tasks_delta_sync_emits_per_resource_changes_and_deletions() {
        // A CalDAV task id is `{href}|{uid}` (native_id = href), so a
        // removed href maps to the cache row directly — no full re-list.
        let mut server = Server::new_async().await;
        let _sync = server
            .mock("REPORT", "/cal/tasks/")
            .match_body(mockito::Matcher::Regex("sync-collection".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/cal/tasks/t1.ics</d:href>
    <d:propstat><d:prop><d:getetag>"t-v2"</d:getetag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/cal/tasks/gone.ics</d:href>
    <d:status>HTTP/1.1 404 Not Found</d:status>
  </d:response>
  <d:sync-token>ST2</d:sync-token>
</d:multistatus>"#,
            )
            .create_async()
            .await;
        let _multiget = server
            .mock("REPORT", "/cal/tasks/")
            .match_body(mockito::Matcher::Regex("calendar-multiget".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/tasks/t1.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"t-v2"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VTODO
UID:task-1@aperio
SUMMARY:Write report
END:VTODO
END:VCALENDAR</c:calendar-data>
    </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#,
            )
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let list_url = format!("{}/cal/tasks/", server.url());
        let cs = adapter
            .get_tasks_delta(&list_url, Some("sync:ST1"))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert!(cs.changes[0].id.ends_with("|task-1@aperio"));
        // The removed href is emitted as a per-resource deletion.
        assert_eq!(cs.deletions, vec!["/cal/tasks/gone.ics".to_string()]);
        assert_eq!(cs.new_token.as_deref(), Some("sync:ST2"));
    }

    #[tokio::test]
    async fn get_contacts_delta_sync_emits_per_resource_changes_and_deletions() {
        let mut server = Server::new_async().await;
        let _sync = server
            .mock("REPORT", "/book/")
            .match_body(mockito::Matcher::Regex("sync-collection".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/book/c1.vcf</d:href>
    <d:propstat><d:prop><d:getetag>"c-v2"</d:getetag></d:prop>
    <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/book/gone.vcf</d:href>
    <d:status>HTTP/1.1 404 Not Found</d:status>
  </d:response>
  <d:sync-token>ST2</d:sync-token>
</d:multistatus>"#,
            )
            .create_async()
            .await;
        let _multiget = server
            .mock("REPORT", "/book/")
            .match_body(mockito::Matcher::Regex("addressbook-multiget".into()))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/book/c1.vcf</d:href>
    <d:propstat><d:prop>
      <d:getetag>"c-v2"</d:getetag>
      <cr:address-data>BEGIN:VCARD
VERSION:3.0
UID:contact-1@aperio
FN:Alice
END:VCARD</cr:address-data>
    </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#,
            )
            .create_async()
            .await;

        let adapter = build_adapter(&server);
        let book_url = format!("{}/book/", server.url());
        let cs = adapter
            .get_contacts_delta(&book_url, Some("sync:ST1"))
            .await
            .unwrap();
        assert!(!cs.full_resync);
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.deletions, vec!["/book/gone.vcf".to_string()]);
        assert_eq!(cs.new_token.as_deref(), Some("sync:ST2"));
    }
}
