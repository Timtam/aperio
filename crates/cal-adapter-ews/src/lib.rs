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

use std::collections::HashMap;
use std::path::PathBuf;
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

use crate::api::SyncedFolderState;
use crate::mapping::to_event;

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
    /// Host-supplied directory the adapter may use to persist
    /// per-account state (the SyncFolderItems cookie + cached
    /// item set). Spliced in at `register_ews` time off the host's
    /// data_dir. Absent on the test path / smoke-test ephemeral
    /// instances; the adapter then keeps state in memory only.
    #[serde(default)]
    pub state_dir: Option<String>,
}

#[derive(Debug)]
pub struct EwsAdapter {
    client: EwsClient,
    capabilities: Vec<Capability>,
    calendars_cache: Mutex<Option<(Vec<Calendar>, chrono::DateTime<chrono::Utc>)>>,
    task_lists_cache: Mutex<Option<(Vec<TaskList>, chrono::DateTime<chrono::Utc>)>>,
    contact_lists_cache:
        Mutex<Option<(Vec<ContactList>, chrono::DateTime<chrono::Utc>)>>,
    /// GAL enumeration is a 39-prefix ResolveNames walk that
    /// burns ~3-5 seconds plus full server round-trips. Cache
    /// the result for half an hour so a second panel open
    /// inside the same session is instant, and a `gal_fetch_lock`
    /// dedupes concurrent first-call attempts (e.g. React
    /// StrictMode's double-invocation in dev) so the server
    /// never sees the parallel double-walk.
    gal_cache:
        Mutex<Option<(Vec<Contact>, chrono::DateTime<chrono::Utc>)>>,
    gal_fetch_lock: Mutex<()>,
    listing_ttl: chrono::Duration,
    gal_ttl: chrono::Duration,
    /// Per-folder cache for the Outlook-style `SyncFolderItems`
    /// read path. Keyed by the Aperio calendar id (the encoded
    /// `<folder-id>|<change-key>` form). Each entry holds every
    /// item the adapter has ever heard about for that folder
    /// plus the server-issued sync cookie so the next round only
    /// pulls deltas.
    ///
    /// Lives in memory by default. When the plugin's
    /// `InitConfig.state_dir` is set, the adapter additionally
    /// loads the prior state at construction time and writes it
    /// back after every successful refresh — so an app restart
    /// resumes from a delta-sync against the persisted cookie
    /// instead of doing a full re-sync of every folder.
    events_sync: Mutex<HashMap<String, SyncedFolderState>>,
    /// Host-supplied directory for persistent state. `None`
    /// disables persistence entirely (test path, smoke-test
    /// ephemeral instances).
    state_dir: Option<PathBuf>,
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
            gal_cache: Mutex::new(None),
            gal_fetch_lock: Mutex::new(()),
            listing_ttl: chrono::Duration::minutes(5),
            gal_ttl: chrono::Duration::minutes(30),
            events_sync: Mutex::new(HashMap::new()),
            state_dir: None,
        }
    }

    /// Attach a persistent state directory. Called by the plugin's
    /// `open_instance` when the host provides one (production
    /// path); leaves the adapter in pure-memory mode when omitted
    /// (test path / smoke-test ephemeral instances).
    ///
    /// On attach the constructor synchronously loads the previous
    /// sync state from `<dir>/events_sync.json` (best-effort:
    /// missing/corrupt → start from scratch + log). Subsequent
    /// successful refreshes write back atomically.
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        // Best-effort load: a missing or malformed file is not
        // fatal — a full re-sync recovers either way. We log so
        // an unexpected deserialize failure shows up in the
        // protocol viewer.
        let path = dir.join("events_sync.json");
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<
                HashMap<String, SyncedFolderState>,
            >(&bytes)
            {
                Ok(restored) => {
                    let count = restored.len();
                    // Replace the empty Mutex with the loaded
                    // map. Safe because nothing has touched
                    // `self.events_sync` yet (we're still inside
                    // `with_state_dir`).
                    self.events_sync = Mutex::new(restored);
                    tracing::debug!(
                        target: "cal_adapter_ews::sync",
                        path = %path.display(),
                        folders = count,
                        "loaded persisted sync state",
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        target: "cal_adapter_ews::sync",
                        path = %path.display(),
                        ?err,
                        "couldn't deserialize sync state; starting fresh",
                    );
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // First run for this account; no state to load.
            }
            Err(err) => {
                tracing::warn!(
                    target: "cal_adapter_ews::sync",
                    path = %path.display(),
                    ?err,
                    "couldn't read sync state; starting fresh",
                );
            }
        }
        self.state_dir = Some(dir);
        self
    }

    /// Snapshot the current sync map to disk. Called from
    /// `refresh_and_read_events` after each successful round so
    /// the next process boot can resume from the persisted
    /// cookie. Atomic via write-then-rename so a crash during
    /// the serialise can't leave a half-written file that fails
    /// to deserialize next boot.
    ///
    /// Best-effort: failures log but don't fail the refresh —
    /// the next round will retry, and the worst case is "next
    /// boot does a full re-sync", same as before persistence
    /// shipped.
    async fn persist_events_sync(
        &self,
        snapshot: &HashMap<String, SyncedFolderState>,
    ) {
        let Some(dir) = self.state_dir.as_ref() else {
            return;
        };
        let path = dir.join("events_sync.json");
        let tmp = dir.join("events_sync.json.tmp");
        let bytes = match serde_json::to_vec(snapshot) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(
                    target: "cal_adapter_ews::sync",
                    ?err,
                    "couldn't serialize sync state",
                );
                return;
            }
        };
        // The actual file I/O is sync; run it on the blocking
        // pool so we don't stall the async runtime on slow disks.
        let path_for_task = path.clone();
        let tmp_for_task = tmp.clone();
        let result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            std::fs::write(&tmp_for_task, &bytes)?;
            std::fs::rename(&tmp_for_task, &path_for_task)?;
            Ok(())
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    target: "cal_adapter_ews::sync",
                    path = %path.display(),
                    ?err,
                    "couldn't write sync state",
                );
            }
            Err(err) => {
                // JoinError from spawn_blocking — pool shutdown
                // or the task panicked. Log + move on.
                tracing::warn!(
                    target: "cal_adapter_ews::sync",
                    ?err,
                    "sync state write task failed",
                );
            }
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

    /// Cached read of the GAL contact set. The actual fetch is
    /// expensive (39 ResolveNames round-trips) so we cache the
    /// whole result for `gal_ttl` (30 min) and dedupe concurrent
    /// callers via `gal_fetch_lock` — without that, React
    /// StrictMode's double-effect-invocation in dev fires two
    /// parallel walks, doubling the load on Exchange (which
    /// then throttles, compounding the wait).
    async fn get_gal_contacts_cached(&self) -> CoreResult<Vec<Contact>> {
        // Fast path: cache hit without ever touching the
        // dedupe lock. Concurrent fast-path readers all walk
        // straight through.
        if let Some(cached) = self.cached_gal().await {
            tracing::debug!(
                target: "cal_adapter_ews::gal",
                cached = cached.len(),
                "GAL cache hit",
            );
            return Ok(cached);
        }
        // Slow path: take the dedupe lock and re-check the cache
        // inside the critical section. The first arrival does
        // the actual fetch and populates the cache; every other
        // caller blocks on `_lock`, then sees the cache hit on
        // re-check and exits.
        let _lock = self.gal_fetch_lock.lock().await;
        if let Some(cached) = self.cached_gal().await {
            return Ok(cached);
        }
        let started = chrono::Utc::now();
        let fresh = contacts::get_contacts(&self.client, contacts::GAL_LIST_ID)
            .await
            .map_err(to_core_error)?;
        *self.gal_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        tracing::debug!(
            target: "cal_adapter_ews::gal",
            count = fresh.len(),
            elapsed_ms = chrono::Utc::now()
                .signed_duration_since(started)
                .num_milliseconds(),
            "GAL fetched and cached",
        );
        Ok(fresh)
    }

    async fn cached_gal(&self) -> Option<Vec<Contact>> {
        let guard = self.gal_cache.lock().await;
        let (items, ts) = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(*ts);
        if age >= chrono::Duration::zero() && age < self.gal_ttl {
            Some(items.clone())
        } else {
            None
        }
    }

    /// Drive a `SyncFolderItems` delta against `calendar_id`,
    /// merge the changes into the per-folder cache, then translate
    /// the cached items into cal-core Events.
    ///
    /// Filtering rules:
    ///   - Recurring masters always pass through (the frontend
    ///     expander computes occurrences within the visible range).
    ///   - Single events pass through only when their start/end
    ///     overlap `[range.start, range.end]`. Cached singles
    ///     outside the window stay in the cache (the next call for
    ///     a different range surfaces them) but don't bloat the
    ///     returned vec.
    ///
    /// On `ErrorInvalidSyncStateData`, we discard the cookie + the
    /// cached items and retry once with a fresh full sync. That
    /// recovers from server-side state expiry / mailbox rebuilds
    /// without surfacing a user-visible error.
    async fn refresh_and_read_events(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> EwsResult<Vec<Event>> {
        // Lock the map briefly to take the current state, run the
        // sync without holding the global lock, then re-acquire to
        // write back. Concurrent calls to refresh_and_read_events
        // for DIFFERENT folders proceed in parallel; same-folder
        // concurrent callers serialise on the second take.
        let prior = {
            let mut guard = self.events_sync.lock().await;
            guard.remove(calendar_id).unwrap_or_default()
        };
        let updated = match api::sync_events_to_completion(
            &self.client,
            calendar_id,
            prior,
        )
        .await
        {
            Ok(s) => s,
            Err(err) if is_sync_state_invalid(&err) => {
                tracing::warn!(
                    target: "cal_adapter_ews::sync",
                    calendar = %calendar_id,
                    "SyncFolderItems cookie invalid; doing a full re-sync",
                );
                api::sync_events_to_completion(
                    &self.client,
                    calendar_id,
                    SyncedFolderState::default(),
                )
                .await?
            }
            Err(err) => return Err(err),
        };

        // Translate cached items to cal-core Events. The map order
        // is non-deterministic; the frontend sorts events anyway,
        // so we don't bother stabilising here.
        let mut out: Vec<Event> = Vec::with_capacity(updated.items.len());
        for (_, item) in updated.items.iter() {
            // Skip Occurrence rows that might have leaked in via
            // older protocol responses — `SyncFolderItems` on a
            // calendar folder shouldn't emit them, but a defensive
            // filter keeps the read path honest.
            if item
                .item_type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("Occurrence"))
                .unwrap_or(false)
            {
                continue;
            }
            // Translate. On the rare per-item failure
            // (RelativeMonthly etc.) log and drop the row rather
            // than failing the whole refresh.
            let ev = match to_event(item.clone(), calendar_id) {
                Ok(ev) => ev,
                Err(err) => {
                    tracing::warn!(
                        target: "cal_adapter_ews::sync",
                        item_id = %item.item_id,
                        ?err,
                        "skipping item: could not translate to cal-core Event",
                    );
                    continue;
                }
            };
            // Range filter applies to singles only. Masters carry
            // a recurrence and the frontend expander handles the
            // window.
            if ev.recurrence.is_none() {
                if ev.end < range.start || ev.start >= range.end {
                    continue;
                }
            }
            // For each modified occurrence on a master, emit a
            // synthetic standalone event at the moved time. The
            // master's EXDATE list (built in `to_event`) already
            // skips the original slot — without this emit the
            // user would see the override-time slot empty.
            //
            // Content (title, body, location, reminders) inherits
            // from the master. Outlook lets the user edit those
            // fields per-occurrence; capturing them would require
            // a per-override GetItem fan-out, deferred. Time
            // changes (the most common kind of override) render
            // correctly with the inherited content.
            if !item.modified_occurrences.is_empty() {
                for ov in &item.modified_occurrences {
                    if ov.end < range.start || ov.start >= range.end {
                        continue;
                    }
                    let mut override_ev = ev.clone();
                    override_ev.id = format!(
                        "{}#override:{}",
                        ev.id,
                        ov.original_start.to_rfc3339(),
                    );
                    override_ev.recurrence = None;
                    override_ev.start = ov.start;
                    override_ev.end = ov.end;
                    override_ev.etag = ov.change_key.clone();
                    out.push(override_ev);
                }
            }
            out.push(ev);
        }

        // Write the updated state back into the map for the next
        // round to consume.
        let snapshot = {
            let mut guard = self.events_sync.lock().await;
            guard.insert(calendar_id.to_string(), updated);
            // Clone the full map so the persist runs without
            // holding the lock — the file write hops over to a
            // blocking task, and we don't want other folders'
            // refreshes blocking on disk I/O.
            guard.clone()
        };
        self.persist_events_sync(&snapshot).await;
        Ok(out)
    }
}

/// EWS reports an expired / unknown sync cookie via the
/// `ErrorInvalidSyncStateData` response code. The SOAP fault
/// surfaces through `check_for_fault` as a `EwsError::Soap`
/// with the code carried in the message. We pattern-match on the
/// substring rather than introducing a typed variant for one
/// case — the alternative happens often enough that the caller
/// branches once.
fn is_sync_state_invalid(err: &EwsError) -> bool {
    matches!(
        err,
        EwsError::Soap { code, .. } if code == "ErrorInvalidSyncStateData",
    )
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
        // SyncFolderItems-backed read path: pull deltas into the
        // per-folder cache, then translate cached ParsedItems to
        // cal-core Events. Masters keep their RRULE; singles are
        // filtered by the date range. Frontend handles the local
        // expansion exactly like CalDAV/iCal.
        self.refresh_and_read_events(calendar_id, range)
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
        if list_id == contacts::GAL_LIST_ID {
            return self.get_gal_contacts_cached().await;
        }
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
            // Skip the synthetic GAL list here — enumerating it
            // would pull the whole directory per keystroke. The
            // `ResolveNames` fan-out below handles GAL hits in
            // O(1) round-trips with server-side prefix matching.
            if list.id == contacts::GAL_LIST_ID {
                continue;
            }
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
        // GAL search runs alongside the personal-folder fan-out.
        // ResolveNames does the matching server-side and caps the
        // result set itself, so a typeahead "ma" against a
        // 5000-entry directory is one round-trip with maybe 100
        // rows back. Per-query failures (`ErrorNameResolutionNoResults`
        // is the common one for nonsense queries) are logged and
        // ignored — the personal-folder hits still surface.
        match contacts::search_gal(&self.client, query).await {
            Ok(gal_hits) => out.extend(gal_hits),
            Err(err) => {
                tracing::debug!(
                    target: "cal_adapter_ews::gal",
                    ?err,
                    "GAL search returned no usable results",
                );
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

    async fn get_contact_photo(
        &self,
        contact_id: &str,
    ) -> CoreResult<Option<cal_core::ContactPhoto>> {
        contacts::get_contact_photo(&self.client, contact_id)
            .await
            .map_err(to_core_error)
    }

    async fn set_contact_photo(
        &self,
        contact_id: &str,
        photo: cal_core::ContactPhoto,
    ) -> CoreResult<()> {
        contacts::set_contact_photo(&self.client, contact_id, photo)
            .await
            .map_err(to_core_error)
    }

    async fn delete_contact_photo(&self, contact_id: &str) -> CoreResult<()> {
        contacts::delete_contact_photo(&self.client, contact_id)
            .await
            .map_err(to_core_error)
    }

    async fn invalidate_contacts_cache(&self) -> CoreResult<()> {
        // Drop both the IPF.Contact folder listing and the
        // expensive GAL pull snapshot. The next
        // `list_contact_lists` / `get_contacts(GAL_LIST_ID)`
        // walks Exchange again — which for the GAL means re-running
        // the a-z ResolveNames enumeration, but that's exactly
        // what the user clicked the button for.
        *self.contact_lists_cache.lock().await = None;
        *self.gal_cache.lock().await = None;
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

#[cfg(test)]
mod state_persistence_tests {
    use super::*;
    use crate::mapping::ParsedItem;

    /// `with_state_dir` should ROUNDTRIP through disk: a freshly
    /// constructed adapter with a pre-populated state file must
    /// surface the same sync_state cookie + cached items the
    /// previous instance wrote out.
    #[tokio::test]
    async fn state_roundtrip_through_disk() {
        // Unique temp dir per run — multiple test invocations in
        // parallel get isolated state.
        let dir = std::env::temp_dir().join(format!(
            "aperio-ews-state-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let creds = BasicCredentials {
            username: "u".into(),
            password: "p".into(),
        };

        // Round 1: adapter writes one folder's worth of state.
        {
            let adapter = EwsAdapter::new(
                "https://example/EWS/Exchange.asmx".into(),
                creds.clone(),
            )
            .with_state_dir(dir.clone());
            let mut state = SyncedFolderState::default();
            state.sync_state = Some("COOKIE-XYZ".into());
            state.items.insert(
                "ITEM-1".into(),
                ParsedItem {
                    item_id: "ITEM-1".into(),
                    subject: "Persisted".into(),
                    ..ParsedItem::default()
                },
            );
            let mut snap = HashMap::new();
            snap.insert("CAL-1".into(), state);
            adapter.persist_events_sync(&snap).await;
        }

        // Round 2: a fresh adapter constructed against the same
        // dir picks up the previous state on load.
        let adapter2 = EwsAdapter::new(
            "https://example/EWS/Exchange.asmx".into(),
            creds,
        )
        .with_state_dir(dir.clone());
        let loaded = adapter2.events_sync.lock().await;
        let restored = loaded.get("CAL-1").expect("CAL-1 state restored");
        assert_eq!(restored.sync_state.as_deref(), Some("COOKIE-XYZ"));
        assert!(restored.items.contains_key("ITEM-1"));
        assert_eq!(restored.items["ITEM-1"].subject, "Persisted");

        // Cleanup — best-effort, doesn't fail the test if the
        // dir lingers.
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt state file must NOT prevent the adapter from
    /// constructing — it should log + start with an empty
    /// `events_sync` so the next refresh does a full re-sync.
    #[tokio::test]
    async fn corrupt_state_file_is_not_fatal() {
        let dir = std::env::temp_dir().join(format!(
            "aperio-ews-state-corrupt-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("events_sync.json"),
            b"{not valid json at all",
        )
        .unwrap();

        let adapter = EwsAdapter::new(
            "https://example/EWS/Exchange.asmx".into(),
            BasicCredentials {
                username: "u".into(),
                password: "p".into(),
            },
        )
        .with_state_dir(dir.clone());
        let map = adapter.events_sync.lock().await;
        assert!(
            map.is_empty(),
            "corrupt state should fall back to empty map",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
