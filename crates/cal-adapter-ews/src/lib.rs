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
pub mod windows_tz;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AttendeeStatus, AuthToken, Calendar, CalendarFeature, Capability, ChangeSet, Contact,
    ContactList, ContactsFeature, ContainerColor, Credentials as CoreCredentials, DateRange,
    Error as CoreError, Event, FreeBusy, NewContact, NewEvent, NewTask, Result as CoreResult, Task,
    TaskList, TasksFeature,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::api::SyncedFolderState;
use crate::mapping::{to_event, ParsedItem};

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
    contact_lists_cache: Mutex<Option<(Vec<ContactList>, chrono::DateTime<chrono::Utc>)>>,
    /// GAL enumeration is a 39-prefix ResolveNames walk that
    /// burns ~3-5 seconds plus full server round-trips. Cache
    /// the result for half an hour so a second panel open
    /// inside the same session is instant, and a `gal_fetch_lock`
    /// dedupes concurrent first-call attempts (e.g. React
    /// StrictMode's double-invocation in dev) so the server
    /// never sees the parallel double-walk.
    gal_cache: Mutex<Option<(Vec<Contact>, chrono::DateTime<chrono::Utc>)>>,
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
    /// Per-folder `SyncFolderItems` cookie for the Tasks delta read,
    /// keyed by the Aperio list id. Unlike `events_sync` this holds only
    /// the cookie (not the item set): the Tasks/Contacts delta is
    /// CTag-style — a cheap IdOnly probe advances the cookie and gates a
    /// full `FindItem` re-read on whether anything changed, so the full
    /// item set never has to live here. In-memory only; a process restart
    /// re-seeds from the host's stored token (see `probe_list_changes`).
    tasks_sync: Mutex<HashMap<String, String>>,
    /// Same as `tasks_sync` for the Contacts delta read (IPF.Contact
    /// folders). The synthetic GAL list has no folder to sync and is
    /// handled separately (returns `Unsupported`).
    contacts_sync: Mutex<HashMap<String, String>>,
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
            tasks_sync: Mutex::new(HashMap::new()),
            contacts_sync: Mutex::new(HashMap::new()),
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
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, SyncedFolderState>>(&bytes)
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
    async fn persist_events_sync(&self, snapshot: &HashMap<String, SyncedFolderState>) {
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
        self.listing_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
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
        *self.gal_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
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
        let updated = match api::sync_events_to_completion(&self.client, calendar_id, prior).await {
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
        let cache_size = updated.items.len();
        let mut translate_failures = 0usize;
        let mut skipped_occurrence = 0usize;
        let mut filtered_out_of_range = 0usize;
        let mut overrides_emitted = 0usize;
        let mut masters_emitted = 0usize;
        let mut singles_emitted = 0usize;
        let mut out: Vec<Event> = Vec::with_capacity(updated.items.len());
        for item in updated.items.values() {
            let before = out.len();
            match emit_item_events(item, calendar_id, range, &mut out) {
                // `out.len() - before - 1` is the count of synthetic
                // override events pushed ahead of the base event.
                Ok(ItemEmit::Master) => {
                    masters_emitted += 1;
                    overrides_emitted += out.len() - before - 1;
                }
                Ok(ItemEmit::Single) => {
                    singles_emitted += 1;
                    overrides_emitted += out.len() - before - 1;
                }
                Ok(ItemEmit::SkippedOccurrence) => skipped_occurrence += 1,
                Ok(ItemEmit::SkippedOutOfRange) => filtered_out_of_range += 1,
                Err(err) => {
                    // Per-item failure (an unsupported recurrence shape, a
                    // missing Start/End): log and drop the row rather than
                    // failing the whole refresh.
                    translate_failures += 1;
                    tracing::warn!(
                        target: "cal_adapter_ews::sync",
                        item_id = %item.item_id,
                        ?err,
                        "skipping item: could not translate to cal-core Event",
                    );
                }
            }
        }

        let cancelled_emitted = out.iter().filter(|e| e.cancelled).count();
        tracing::info!(
            target: "cal_adapter_ews::sync",
            calendar = %calendar_id,
            cache_size,
            singles_emitted,
            masters_emitted,
            overrides_emitted,
            cancelled_emitted,
            filtered_out_of_range,
            skipped_occurrence,
            translate_failures,
            "EWS get_events: cache → cal-core",
        );

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

    /// Incremental sibling of [`refresh_and_read_events`] backing
    /// [`CalendarFeature::get_events_delta`]. Drives one `SyncFolderItems`
    /// delta, persists the merged per-folder state, and hands the host a
    /// `ChangeSet` to fold into its snapshot cache.
    ///
    /// The host replaces wholesale whenever it has no prior token OR we
    /// flag `full_resync`; in both cases `changes` must be the COMPLETE
    /// in-range set. We declare a full resync when our own per-folder
    /// state had no cookie (cold), when the host passed no `since_token`,
    /// or when the server invalidated the cookie and we re-drained from
    /// scratch. Otherwise the result is purely incremental: changed items
    /// translated (masters keep their RRULE plus synthetic in-range
    /// overrides) and the deleted EWS ItemIds verbatim — those are the
    /// provider-native ids the host removes by `native_id`, which fans a
    /// master's deletion out to its cached occurrence overrides.
    async fn refresh_events_delta(
        &self,
        calendar_id: &str,
        range: DateRange,
        since_token: Option<&str>,
    ) -> EwsResult<ChangeSet<Event>> {
        let prior = {
            let mut guard = self.events_sync.lock().await;
            guard.remove(calendar_id).unwrap_or_default()
        };
        // Host-authoritative cursor. The snapshot cache the host folds this
        // delta into is keyed to `since_token`, so we MUST drain from THAT,
        // not from our own persisted cookie. `get_events` (the reminder
        // scan's read path) shares this same per-folder state and advances +
        // persists the cookie on its own schedule; draining from our cookie
        // would then silently skip every change that read already consumed
        // but the host never saw — leaving edited events stuck at their old
        // time until a full resync. We keep the accumulated `items` (so a
        // changed recurring master still emits with its overrides) and only
        // reset the cursor to the host's. With no `since_token`, or when our
        // own snapshot is cold (no cookie/items to merge a delta into), the
        // host replaces wholesale, so we drain the COMPLETE folder from
        // scratch instead.
        let adapter_warm = prior.sync_state.is_some();
        let mut force_full = since_token.is_none() || !adapter_warm;
        let seed = match since_token {
            Some(tok) if adapter_warm => SyncedFolderState {
                sync_state: Some(tok.to_string()),
                ..prior
            },
            _ => SyncedFolderState::default(),
        };
        let (updated, changed_ids, deleted_ids) =
            match api::sync_events_delta(&self.client, calendar_id, seed).await {
                Ok(t) => t,
                Err(err) if is_sync_state_invalid(&err) => {
                    tracing::warn!(
                        target: "cal_adapter_ews::sync",
                        calendar = %calendar_id,
                        "SyncFolderItems cookie invalid; doing a full re-sync",
                    );
                    // A from-scratch re-drain returns the whole folder;
                    // flag a full resync so the host wipes rows that
                    // vanished while the cookie was stale instead of
                    // merging a partial set.
                    force_full = true;
                    api::sync_events_delta(&self.client, calendar_id, SyncedFolderState::default())
                        .await?
                }
                Err(err) => return Err(err),
            };

        // Folder-complete emit: the host caches the whole folder and
        // serves every view range from that single snapshot, so we emit
        // every item regardless of the caller's `range`. An unbounded
        // range disables the single/override range filter inside
        // `emit_item_events` without duplicating its recurrence logic.
        // (Masters always pass anyway; the frontend expander windows them.)
        let full = DateRange::new(
            chrono::DateTime::<chrono::Utc>::MIN_UTC,
            chrono::DateTime::<chrono::Utc>::MAX_UTC,
        );
        let change_set = if force_full {
            let mut changes = Vec::with_capacity(updated.items.len());
            for item in updated.items.values() {
                emit_into(item, calendar_id, full, &mut changes);
            }
            ChangeSet {
                changes,
                deletions: Vec::new(),
                new_token: updated.sync_state.clone(),
                full_resync: true,
                // EWS SyncFolderItems keeps the whole folder in `items`,
                // and we emit it unfiltered above — the host may treat
                // this snapshot as covering any range (unbounded window).
                complete: true,
            }
        } else {
            let mut changes = Vec::new();
            for id in &changed_ids {
                if let Some(item) = updated.items.get(id) {
                    emit_into(item, calendar_id, full, &mut changes);
                }
            }
            ChangeSet {
                changes,
                deletions: deleted_ids,
                new_token: updated.sync_state.clone(),
                full_resync: false,
                // Incremental, but still folder-complete: the cache holds
                // the whole folder from the prior full sync and this merge
                // keeps it current.
                complete: true,
            }
        };

        // Persist the merged state for the next round (mirrors
        // refresh_and_read_events: clone the map, then write off-lock).
        let snapshot = {
            let mut guard = self.events_sync.lock().await;
            guard.insert(calendar_id.to_string(), updated);
            guard.clone()
        };
        self.persist_events_sync(&snapshot).await;

        tracing::info!(
            target: "cal_adapter_ews::sync",
            calendar = %calendar_id,
            full_resync = change_set.full_resync,
            changes = change_set.changes.len(),
            deletions = change_set.deletions.len(),
            range_start = %range.start.to_rfc3339(),
            range_end = %range.end.to_rfc3339(),
            "EWS get_events_delta: drain → ChangeSet (folder-complete)",
        );
        Ok(change_set)
    }

    /// Shared probe logic for the Tasks/Contacts delta read. Runs the
    /// cheap IdOnly `SyncFolderItems` probe against `list_id`, stores the
    /// fresh cookie in `sync_map`, and reports whether the caller must do
    /// a full `FindItem` re-read.
    ///
    /// The probe is seeded from our in-memory cookie when we have one,
    /// else from the host's `since_token` — so a process restart (empty
    /// map) resumes from the host's stored cookie and skips the full read
    /// when nothing changed. A full read is forced when the folder changed,
    /// the cookie aged out, or `since_token` is `None` (the host then
    /// replaces wholesale and `changes` must be the complete set).
    async fn probe_list_changes(
        &self,
        sync_map: &Mutex<HashMap<String, String>>,
        list_id: &str,
        since_token: Option<&str>,
    ) -> EwsResult<(bool, String)> {
        let seed = {
            let guard = sync_map.lock().await;
            guard
                .get(list_id)
                .cloned()
                .or_else(|| since_token.map(String::from))
        };
        let outcome = match api::probe_folder_sync(&self.client, list_id, seed.as_deref()).await {
            Ok(o) => o,
            Err(err) if is_sync_state_invalid(&err) => {
                tracing::warn!(
                    target: "cal_adapter_ews::sync",
                    list = %list_id,
                    "SyncFolderItems probe cookie invalid; re-probing from scratch",
                );
                // Cold re-probe; a stale cookie means the cached snapshot
                // may be wrong, so force the full re-read.
                let mut o = api::probe_folder_sync(&self.client, list_id, None).await?;
                o.changed = true;
                o
            }
            Err(err) => return Err(err),
        };
        sync_map
            .lock()
            .await
            .insert(list_id.to_string(), outcome.sync_state.clone());
        let need_full = outcome.changed || since_token.is_none();
        Ok((need_full, outcome.sync_state))
    }
}

/// Outcome of translating one cached item, reported back so the full
/// read path can keep its diagnostic counters.
enum ItemEmit {
    /// A recurring master was emitted (plus any in-range overrides).
    Master,
    /// A single event was emitted.
    Single,
    /// A defensive-filtered `Occurrence` row — nothing emitted.
    SkippedOccurrence,
    /// A single event whose slot fell entirely outside `range`.
    SkippedOutOfRange,
}

/// Translate one cached [`ParsedItem`] into cal-core events, pushing them
/// onto `out`, and report what happened.
///
/// Shared by the full read ([`EwsAdapter::refresh_and_read_events`]) and
/// the incremental delta ([`EwsAdapter::refresh_events_delta`]) so both
/// surface series identically. Emission rules:
///   - `Occurrence`-typed rows are dropped defensively — `SyncFolderItems`
///     on a calendar folder shouldn't surface them.
///   - Recurring masters always pass (the frontend expander handles the
///     visible window), and each in-range [`ModifiedOccurrence`] is emitted
///     as a synthetic standalone event at the moved time, inheriting the
///     master's content. The master's EXDATE list (built in `to_event`)
///     already vacates the original slot, so the expander doesn't double-
///     render. The synthetic id is derived from the master's, so it shares
///     the master's `native_id` host-side and is purged together with it.
///   - Single events pass only when they overlap `range`.
///
/// [`ModifiedOccurrence`]: crate::mapping::ModifiedOccurrence
fn emit_item_events(
    item: &ParsedItem,
    calendar_id: &str,
    range: DateRange,
    out: &mut Vec<Event>,
) -> EwsResult<ItemEmit> {
    if item
        .item_type
        .as_deref()
        .map(|t| t.eq_ignore_ascii_case("Occurrence"))
        .unwrap_or(false)
    {
        return Ok(ItemEmit::SkippedOccurrence);
    }
    let ev = to_event(item.clone(), calendar_id)?;
    // Range filter applies to singles only; masters carry a recurrence and
    // the frontend expander handles the window.
    if ev.recurrence.is_none() && (ev.end < range.start || ev.start >= range.end) {
        return Ok(ItemEmit::SkippedOutOfRange);
    }
    if !item.modified_occurrences.is_empty() {
        for ov in &item.modified_occurrences {
            if ov.end < range.start || ov.start >= range.end {
                continue;
            }
            let mut override_ev = ev.clone();
            override_ev.id = format!("{}#override:{}", ev.id, ov.original_start.to_rfc3339());
            override_ev.recurrence = None;
            override_ev.start = ov.start;
            override_ev.end = ov.end;
            override_ev.etag = ov.change_key.clone();
            // A cancelled occurrence (organizer withdrew just this instance)
            // arrives as a cancelled exception item; its cancelled state is
            // resolved by the per-override GetItem enrichment. Carry it (and
            // the master's own cancelled state) onto the emitted override so a
            // single cancelled occurrence is dimmed + announced.
            override_ev.cancelled = ev.cancelled || ov.cancelled;
            out.push(override_ev);
        }
    }
    // TEMP DIAG (revert once Toni's un-flagged EWS cancellation is diagnosed):
    // log every in-range emitted event's RAW cancellation signals at INFO so a
    // normal (INFO-level) user log reveals exactly how Exchange represents a
    // cancelled meeting we're failing to flag — the `debug`-level probe was
    // invisible because the app ships at INFO. For recurring masters we also
    // dump the RRULE + EXDATEs + moved-occurrence origins, so a cancelled
    // occurrence (which the adapter logs at the master's anchor, not the
    // occurrence date) can be traced to how the series encoded it.
    let rrule = ev
        .recurrence
        .as_ref()
        .map(|r| r.rrule.clone())
        .unwrap_or_default();
    let exdates = ev
        .recurrence
        .as_ref()
        .map(|r| {
            r.exceptions
                .iter()
                .map(|d| d.to_rfc3339())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let modified_origins = item
        .modified_occurrences
        .iter()
        .map(|m| m.original_start.to_rfc3339())
        .collect::<Vec<_>>()
        .join(",");
    tracing::info!(
        target: "cal_adapter_ews::sync",
        id = %ev.id,
        start = %ev.start,
        is_cancelled_raw = item.cancelled,
        appointment_state = ?item.appointment_state,
        resolved_cancelled = ev.cancelled,
        rrule = %rrule,
        exdates = %exdates,
        modified_origins = %modified_origins,
        subject = %item.subject,
        "TEMP ews emitted event cancelled-state",
    );
    let outcome = if ev.recurrence.is_some() {
        ItemEmit::Master
    } else {
        ItemEmit::Single
    };
    out.push(ev);
    Ok(outcome)
}

/// `emit_item_events` for callers that don't track counters: a per-item
/// translation failure is logged and the row dropped, never fatal to the
/// surrounding drain.
fn emit_into(item: &ParsedItem, calendar_id: &str, range: DateRange, out: &mut Vec<Event>) {
    if let Err(err) = emit_item_events(item, calendar_id, range, out) {
        tracing::warn!(
            target: "cal_adapter_ews::sync",
            item_id = %item.item_id,
            ?err,
            "delta: skipping item that could not translate to cal-core Event",
        );
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
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
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
            tracing::debug!(
                target: "cal_adapter_ews::sync",
                count = cached.len(),
                "EwsAdapter::list_calendars cache hit",
            );
            return Ok(cached);
        }
        let result = api::list_calendars(&self.client).await;
        match &result {
            Ok(fresh) => tracing::info!(
                target: "cal_adapter_ews::sync",
                count = fresh.len(),
                "EwsAdapter::list_calendars fetched",
            ),
            Err(err) => tracing::warn!(
                target: "cal_adapter_ews::sync",
                ?err,
                "EwsAdapter::list_calendars failed",
            ),
        }
        let fresh = result.map_err(to_core_error)?;
        *self.calendars_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> CoreResult<Vec<Event>> {
        // INFO-level entry log so a user diagnosing "EWS events
        // don't show up" can immediately tell whether the trait
        // is even being hit — separates "frontend isn't asking"
        // from "frontend is asking but sync fails" / "sync ok but
        // 0 events emitted".
        tracing::info!(
            target: "cal_adapter_ews::sync",
            calendar = %calendar_id,
            range_start = %range.start.to_rfc3339(),
            range_end = %range.end.to_rfc3339(),
            "EwsAdapter::get_events called",
        );
        // SyncFolderItems-backed read path: pull deltas into the
        // per-folder cache, then translate cached ParsedItems to
        // cal-core Events. Masters keep their RRULE; singles are
        // filtered by the date range. Frontend handles the local
        // expansion exactly like CalDAV/iCal.
        let result = self.refresh_and_read_events(calendar_id, range).await;
        if let Err(ref err) = result {
            tracing::warn!(
                target: "cal_adapter_ews::sync",
                calendar = %calendar_id,
                ?err,
                "EwsAdapter::get_events failed",
            );
        }
        result.map_err(to_core_error)
    }

    /// Host-driven incremental read (CACHE-8). Backs the snapshot cache's
    /// per-calendar delta refresh via the same `SyncFolderItems` cookie
    /// the full read uses, so the host only re-pulls the rows Exchange
    /// reports as touched. See [`EwsAdapter::refresh_events_delta`] for
    /// the full-resync vs. incremental decision and the deletion/native-id
    /// contract.
    async fn get_events_delta(
        &self,
        calendar_id: &str,
        range: DateRange,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Event>> {
        tracing::info!(
            target: "cal_adapter_ews::sync",
            calendar = %calendar_id,
            has_token = since_token.is_some(),
            "EwsAdapter::get_events_delta called",
        );
        let result = self
            .refresh_events_delta(calendar_id, range, since_token)
            .await;
        if let Err(ref err) = result {
            tracing::warn!(
                target: "cal_adapter_ews::sync",
                calendar = %calendar_id,
                ?err,
                "EwsAdapter::get_events_delta failed",
            );
        }
        result.map_err(to_core_error)
    }

    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> CoreResult<Event> {
        api::create_event(&self.client, calendar_id, event)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        api::update_event(&self.client, &event)
            .await
            .map_err(to_core_error)
    }

    async fn delete_event(&self, event_id: &str, send_cancellations: bool) -> CoreResult<()> {
        api::delete_event(&self.client, event_id, send_cancellations)
            .await
            .map_err(to_core_error)
    }

    async fn rename_calendar(&self, calendar_id: &str, new_name: &str) -> CoreResult<()> {
        api::rename_calendar(&self.client, calendar_id, new_name)
            .await
            .map_err(to_core_error)?;
        // Drop the cached calendar list so the next list_calendars
        // round-trip picks up the new display name. The calendar id
        // itself is stable (just the folder EntryID), so the rename's
        // server-side ChangeKey bump doesn't affect it.
        *self.calendars_cache.lock().await = None;
        Ok(())
    }

    async fn get_free_busy(&self, emails: &[&str], range: DateRange) -> CoreResult<Vec<FreeBusy>> {
        api::query_free_busy(&self.client, emails, range)
            .await
            .map_err(to_core_error)
    }

    async fn current_user_email(&self) -> CoreResult<Option<String>> {
        // EWS has no cheap "who am I" call (Autodiscover/GetUserSettings
        // is heavyweight). The configured login is the SMTP address on
        // the vast majority of Exchange/Outlook setups; when it isn't an
        // email (e.g. DOMAIN\user), we can't safely match it against
        // attendee SMTP addresses, so we report None and the RSVP UI
        // stays hidden rather than guessing wrong.
        let username = self.client.credentials.username.trim();
        Ok((username.contains('@') && !username.contains('\\')).then(|| username.to_string()))
    }

    async fn respond_to_event(
        &self,
        event_id: &str,
        status: AttendeeStatus,
        send_response: bool,
    ) -> CoreResult<()> {
        api::respond_to_event(&self.client, event_id, status, send_response)
            .await
            .map_err(to_core_error)
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
        *self.task_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        tasks::get_tasks(&self.client, list_id)
            .await
            .map_err(to_core_error)
    }

    /// Host-driven incremental task read (CACHE-8). EWS has no per-item
    /// task delta we can merge cleanly (the cal-core id embeds the
    /// rotating ChangeKey), so this is CTag-style instead: a cheap IdOnly
    /// `SyncFolderItems` probe gates a full `FindItem` re-read. Unchanged
    /// folders return an empty no-op ChangeSet; a changed folder (or the
    /// bootstrap / a token-less call) returns the full task set as a
    /// `full_resync` so the host replaces wholesale.
    async fn get_tasks_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Task>> {
        let (need_full, cookie) = self
            .probe_list_changes(&self.tasks_sync, list_id, since_token)
            .await
            .map_err(to_core_error)?;
        if need_full {
            let tasks = tasks::get_tasks(&self.client, list_id)
                .await
                .map_err(to_core_error)?;
            Ok(ChangeSet {
                changes: tasks,
                deletions: Vec::new(),
                new_token: Some(cookie),
                full_resync: true,
                complete: false,
            })
        } else {
            Ok(ChangeSet {
                changes: Vec::new(),
                deletions: Vec::new(),
                new_token: Some(cookie),
                full_resync: false,
                complete: false,
            })
        }
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> CoreResult<Task> {
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

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
        tasks::rename_task_list(&self.client, list_id, new_name)
            .await
            .map_err(to_core_error)?;
        // Same cache invalidation pattern as `rename_calendar`: drop
        // the cached list so the next list_task_lists round-trip
        // picks up the new display name.
        *self.task_lists_cache.lock().await = None;
        Ok(())
    }

    async fn create_task_list(&self, name: &str, _parent_id: Option<&str>) -> CoreResult<TaskList> {
        let created = tasks::create_task_list(&self.client, name)
            .await
            .map_err(to_core_error)?;
        // Drop the cache so the freshly-created folder shows up on the
        // next listing round-trip.
        *self.task_lists_cache.lock().await = None;
        Ok(created)
    }

    async fn delete_task_list(&self, list_id: &str) -> CoreResult<()> {
        tasks::delete_task_list(&self.client, list_id)
            .await
            .map_err(to_core_error)?;
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
        *self.contact_lists_cache.lock().await = Some((fresh.clone(), chrono::Utc::now()));
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

    /// Host-driven incremental contact read (CACHE-8). Same CTag-style
    /// probe-gated full re-read as `get_tasks_delta`. The synthetic GAL
    /// list is a ResolveNames walk with no folder to sync, so it returns
    /// `Unsupported` and the host falls back to a full read.
    async fn get_contacts_delta(
        &self,
        list_id: &str,
        since_token: Option<&str>,
    ) -> CoreResult<ChangeSet<Contact>> {
        if list_id == contacts::GAL_LIST_ID {
            return Err(CoreError::Unsupported(
                "the EWS GAL (ResolveNames) has no per-folder delta sync".into(),
            ));
        }
        let (need_full, cookie) = self
            .probe_list_changes(&self.contacts_sync, list_id, since_token)
            .await
            .map_err(to_core_error)?;
        if need_full {
            let contacts = contacts::get_contacts(&self.client, list_id)
                .await
                .map_err(to_core_error)?;
            Ok(ChangeSet {
                changes: contacts,
                deletions: Vec::new(),
                new_token: Some(cookie),
                full_resync: true,
                complete: false,
            })
        } else {
            Ok(ChangeSet {
                changes: Vec::new(),
                deletions: Vec::new(),
                new_token: Some(cookie),
                full_resync: false,
                complete: false,
            })
        }
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

    async fn create_contact(&self, list_id: &str, contact: NewContact) -> CoreResult<Contact> {
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

    async fn rename_contact_list(&self, list_id: &str, new_name: &str) -> CoreResult<()> {
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
            "ErrorAccessDenied"
            | "ErrorInvalidAccessToken"
            | "ErrorPasswordExpired"
            | "ErrorADUnavailable"
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
            let adapter =
                EwsAdapter::new("https://example/EWS/Exchange.asmx".into(), creds.clone())
                    .with_state_dir(dir.clone());
            let mut state = SyncedFolderState {
                sync_state: Some("COOKIE-XYZ".into()),
                ..Default::default()
            };
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
        let adapter2 = EwsAdapter::new("https://example/EWS/Exchange.asmx".into(), creds)
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
        std::fs::write(dir.join("events_sync.json"), b"{not valid json at all").unwrap();

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

#[cfg(test)]
mod delta_read_tests {
    //! `get_events_delta` against a mocked `SyncFolderItems` server.
    //!
    //! Each drain is two POSTs — the SyncFolderItems page, then the
    //! GetItem body/recurrence enrichment fan-out — and mockito serves
    //! `.expect(1)` mocks in FIFO creation order, so we lay the four
    //! POSTs of a cold-then-warm sequence out as four ordered mocks.
    use super::*;
    use mockito::Server;

    fn creds() -> BasicCredentials {
        BasicCredentials {
            username: "alice".into(),
            password: "pw".into(),
        }
    }

    fn range() -> DateRange {
        DateRange::new(
            "2026-05-20T00:00:00Z".parse().unwrap(),
            "2026-05-21T00:00:00Z".parse().unwrap(),
        )
    }

    /// A SyncFolderItems page wrapping the supplied `<m:Changes>` body,
    /// terminating the drain (`IncludesLastItemInRange=true`).
    fn sync_page(cookie: &str, changes: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>{cookie}</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes>{changes}</m:Changes>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#
        )
    }

    /// A minimal GetItem enrichment response carrying just the ItemIds —
    /// enough to flip `detail_fetched` without changing the base fields.
    fn getitem_body(items: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items>{items}</m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#
        )
    }

    fn single_create(id: &str, ck: &str, subject: &str, start: &str, end: &str) -> String {
        format!(
            r#"<t:Create><t:CalendarItem>
                 <t:ItemId Id="{id}" ChangeKey="{ck}"/>
                 <t:Subject>{subject}</t:Subject>
                 <t:Start>{start}</t:Start>
                 <t:End>{end}</t:End>
                 <t:CalendarItemType>Single</t:CalendarItemType>
               </t:CalendarItem></t:Create>"#
        )
    }

    #[tokio::test]
    async fn cold_call_is_full_resync_then_warm_call_is_incremental() {
        let mut server = Server::new_async().await;

        // ── Cold drain: two creates, then enrichment of both. ──
        let cold_changes = format!(
            "{}{}",
            single_create(
                "A",
                "CKA1",
                "Alpha",
                "2026-05-20T08:00:00Z",
                "2026-05-20T09:00:00Z"
            ),
            single_create(
                "B",
                "CKB1",
                "Bravo",
                "2026-05-20T10:00:00Z",
                "2026-05-20T11:00:00Z"
            ),
        );
        let _cold_sync = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(sync_page("COOKIE-1", &cold_changes))
            .expect(1)
            .create_async()
            .await;
        let _cold_enrich = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(getitem_body(
                r#"<t:CalendarItem><t:ItemId Id="A" ChangeKey="CKA1"/></t:CalendarItem>
                   <t:CalendarItem><t:ItemId Id="B" ChangeKey="CKB1"/></t:CalendarItem>"#,
            ))
            .expect(1)
            .create_async()
            .await;

        // ── Warm drain: update A (new ChangeKey), delete B, enrich A. ──
        let warm_changes = r#"<t:Update><t:CalendarItem>
                 <t:ItemId Id="A" ChangeKey="CKA2"/>
                 <t:Subject>Alpha v2</t:Subject>
                 <t:Start>2026-05-20T08:30:00Z</t:Start>
                 <t:End>2026-05-20T09:30:00Z</t:End>
                 <t:CalendarItemType>Single</t:CalendarItemType>
               </t:CalendarItem></t:Update>
               <t:Delete><t:ItemId Id="B"/></t:Delete>"#
            .to_string();
        let _warm_sync = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(sync_page("COOKIE-2", &warm_changes))
            .expect(1)
            .create_async()
            .await;
        let _warm_enrich = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(getitem_body(
                r#"<t:CalendarItem><t:ItemId Id="A" ChangeKey="CKA2"/></t:CalendarItem>"#,
            ))
            .expect(1)
            .create_async()
            .await;

        let adapter = EwsAdapter::new(server.url(), creds());

        // Cold call: no token → full resync with the complete in-range set.
        let cold = adapter
            .get_events_delta("FA|FCK", range(), None)
            .await
            .unwrap();
        assert!(cold.full_resync, "first (token-less) call is a full resync");
        assert_eq!(cold.changes.len(), 2);
        assert!(cold.deletions.is_empty());
        assert_eq!(cold.new_token.as_deref(), Some("COOKIE-1"));
        let mut ids: Vec<&str> = cold.changes.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["S:A|CKA1", "S:B|CKB1"]);

        // Warm call: prior cookie → incremental. Only A changed; B is a
        // deletion carrying the raw native ItemId.
        let warm = adapter
            .get_events_delta("FA|FCK", range(), Some("COOKIE-1"))
            .await
            .unwrap();
        assert!(!warm.full_resync, "warm call with a token is incremental");
        assert_eq!(warm.changes.len(), 1);
        assert_eq!(warm.changes[0].id, "S:A|CKA2");
        assert_eq!(warm.changes[0].title, "Alpha v2");
        // Deletion is the bare EWS ItemId — matches the cached row's
        // native_id host-side.
        assert_eq!(warm.deletions, vec!["B".to_string()]);
        assert_eq!(warm.new_token.as_deref(), Some("COOKIE-2"));
    }

    /// Regression: the adapter's own SyncFolderItems cookie can be advanced
    /// (and persisted) independently by the `get_events` read path — the
    /// reminder scan drains the same per-folder state on its own schedule.
    /// The host-facing delta must therefore re-drain from the HOST's
    /// `since_token`, never from our advanced cookie; otherwise a change that
    /// read already consumed but never wrote to the host cache is lost (an
    /// edited Outlook event stuck at its old time until a full resync). Here
    /// the adapter sits at `ADAPTER-AHEAD` while the host is still at
    /// `HOST-BEHIND`; the delta must hit the server with `HOST-BEHIND`.
    #[tokio::test]
    async fn delta_drains_from_host_token_not_the_adapters_advanced_cookie() {
        use mockito::Matcher;
        let mut server = Server::new_async().await;

        // The SyncFolderItems page is served ONLY when the request carries the
        // host's cursor. If the code regressed to draining from the adapter's
        // own (advanced) cookie, this matcher fails → the mock 501s → the call
        // errors → the test fails loudly.
        let update = r#"<t:Update><t:CalendarItem>
                 <t:ItemId Id="A" ChangeKey="CKA2"/>
                 <t:Subject>Alpha v2</t:Subject>
                 <t:Start>2026-05-20T08:30:00Z</t:Start>
                 <t:End>2026-05-20T09:30:00Z</t:End>
                 <t:CalendarItemType>Single</t:CalendarItemType>
               </t:CalendarItem></t:Update>"#;
        let _sync = server
            .mock("POST", "/")
            .match_body(Matcher::AllOf(vec![
                Matcher::Regex("SyncFolderItems".into()),
                Matcher::Regex("HOST-BEHIND".into()),
            ]))
            .with_status(200)
            .with_body(sync_page("COOKIE-3", update))
            .expect(1)
            .create_async()
            .await;
        let _enrich = server
            .mock("POST", "/")
            .match_body(Matcher::Regex("GetItem".into()))
            .with_status(200)
            .with_body(getitem_body(
                r#"<t:CalendarItem><t:ItemId Id="A" ChangeKey="CKA2"/></t:CalendarItem>"#,
            ))
            .expect(1)
            .create_async()
            .await;

        let adapter = EwsAdapter::new(server.url(), creds());
        // Seed the per-folder state as if a prior get_events drain advanced
        // the cookie past the change and persisted it, with the OLD item still
        // cached.
        {
            let mut guard = adapter.events_sync.lock().await;
            let mut state = SyncedFolderState {
                sync_state: Some("ADAPTER-AHEAD".into()),
                ..Default::default()
            };
            state.items.insert(
                "A".into(),
                ParsedItem {
                    item_id: "A".into(),
                    subject: "Alpha".into(),
                    ..ParsedItem::default()
                },
            );
            guard.insert("FA|FCK".into(), state);
        }

        let cs = adapter
            .get_events_delta("FA|FCK", range(), Some("HOST-BEHIND"))
            .await
            .expect("delta must re-drain from the host token, not the adapter cookie");
        assert!(!cs.full_resync, "host has a token → incremental");
        assert_eq!(cs.changes.len(), 1);
        assert_eq!(cs.changes[0].id, "S:A|CKA2");
        assert_eq!(cs.changes[0].title, "Alpha v2");
        assert_eq!(cs.new_token.as_deref(), Some("COOKIE-3"));
    }
}

#[cfg(test)]
mod tasks_contacts_delta_tests {
    //! `get_tasks_delta` / `get_contacts_delta` — the CTag-style probe
    //! that gates a full `FindItem` re-read. EWS posts everything to one
    //! endpoint, so the request sequence is laid out as ordered `.expect(1)`
    //! mocks (FIFO), like the existing event-drain test.
    use super::*;
    use mockito::Server;

    fn creds() -> BasicCredentials {
        BasicCredentials {
            username: "alice".into(),
            password: "pw".into(),
        }
    }

    /// An IdOnly SyncFolderItems probe page that terminates the drain.
    fn probe_page(cookie: &str, changes_inner: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>{cookie}</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes>{changes_inner}</m:Changes>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#
        )
    }

    const ONE_TASK_FIND: &str = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:FindItemResponse><m:ResponseMessages>
    <m:FindItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:RootFolder TotalItemsInView="1"><t:Items>
        <t:Task>
          <t:ItemId Id="T1" ChangeKey="K1"/>
          <t:Subject>Buy milk</t:Subject>
          <t:Status>NotStarted</t:Status>
        </t:Task>
      </t:Items></m:RootFolder>
    </m:FindItemResponseMessage>
  </m:ResponseMessages></m:FindItemResponse></s:Body>
</s:Envelope>"#;

    #[tokio::test]
    async fn tasks_delta_probe_gates_full_read() {
        let mut server = Server::new_async().await;
        // POST 1 — cold probe with a Create → changed.
        let _probe1 = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(probe_page(
                "TC1",
                r#"<t:Create><t:Task><t:ItemId Id="T1" ChangeKey="K1"/></t:Task></t:Create>"#,
            ))
            .expect(1)
            .create_async()
            .await;
        // POST 2 — the gated full FindItem read.
        let _find = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(ONE_TASK_FIND)
            .expect(1)
            .create_async()
            .await;
        // POST 3 — warm probe, no changes → no-op.
        let _probe2 = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(probe_page("TC2", ""))
            .expect(1)
            .create_async()
            .await;

        let adapter = EwsAdapter::new(server.url(), creds());

        // Cold (no token): probe sees a change → full read → full_resync.
        let cold = adapter.get_tasks_delta("TF|TCK", None).await.unwrap();
        assert!(cold.full_resync);
        assert_eq!(cold.changes.len(), 1);
        assert_eq!(cold.changes[0].title, "Buy milk");
        assert_eq!(cold.new_token.as_deref(), Some("TC1"));

        // Warm (prior cookie): probe sees nothing → empty no-op, fresh cookie.
        let warm = adapter
            .get_tasks_delta("TF|TCK", Some("TC1"))
            .await
            .unwrap();
        assert!(!warm.full_resync);
        assert!(warm.changes.is_empty());
        assert_eq!(warm.new_token.as_deref(), Some("TC2"));
    }

    #[tokio::test]
    async fn contacts_delta_gal_is_unsupported() {
        // The GAL is a ResolveNames walk with no folder cookie — it must
        // surface Unsupported so the host falls back to a full read.
        let server = Server::new_async().await;
        let adapter = EwsAdapter::new(server.url(), creds());
        let err = adapter
            .get_contacts_delta(crate::contacts::GAL_LIST_ID, None)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Unsupported(_)));
    }
}

#[cfg(test)]
mod occurrence_cancellation_tests {
    use super::*;
    use crate::mapping::{
        EwsRecurrence, EwsRecurrencePattern, EwsRecurrenceRange, ModifiedOccurrence, ParsedItem,
    };
    use cal_core::DateRange;
    use chrono::{DateTime, Utc};

    fn dt(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    /// A recurring master (not itself cancelled) with two exception overrides:
    /// one the organizer cancelled, one merely moved. The cancelled occurrence's
    /// emitted override must carry `cancelled=true` (so it is dimmed + announced);
    /// the moved one must stay `false`; the master stays a non-cancelled series.
    #[test]
    fn cancelled_occurrence_override_is_emitted_cancelled() {
        let master = ParsedItem {
            item_id: "M1".into(),
            subject: "Austausch Frank - Toni".into(),
            start: Some(dt("2026-07-23T12:00:00Z")),
            end: Some(dt("2026-07-23T12:30:00Z")),
            is_recurring: true,
            recurrence: Some(EwsRecurrence {
                pattern: EwsRecurrencePattern::Daily { interval: 14 },
                range: EwsRecurrenceRange::NoEnd,
            }),
            modified_occurrences: vec![
                ModifiedOccurrence {
                    item_id: "OCC-CANCELLED".into(),
                    change_key: None,
                    start: dt("2026-08-06T12:00:00Z"),
                    end: dt("2026-08-06T12:30:00Z"),
                    original_start: dt("2026-08-06T12:00:00Z"),
                    cancelled: true,
                },
                ModifiedOccurrence {
                    item_id: "OCC-MOVED".into(),
                    change_key: None,
                    start: dt("2026-08-20T15:00:00Z"),
                    end: dt("2026-08-20T15:30:00Z"),
                    original_start: dt("2026-08-20T12:00:00Z"),
                    cancelled: false,
                },
            ],
            ..Default::default()
        };
        let range = DateRange {
            start: dt("2026-08-01T00:00:00Z"),
            end: dt("2026-09-01T00:00:00Z"),
        };
        let mut out = Vec::new();
        emit_item_events(&master, "cal", range, &mut out).unwrap();

        let aug6 = out
            .iter()
            .find(|e| e.start == dt("2026-08-06T12:00:00Z"))
            .expect("cancelled occurrence emitted");
        assert!(
            aug6.cancelled,
            "organizer-cancelled occurrence must be marked cancelled"
        );

        let aug20 = out
            .iter()
            .find(|e| e.start == dt("2026-08-20T15:00:00Z"))
            .expect("moved occurrence emitted");
        assert!(
            !aug20.cancelled,
            "a merely moved occurrence must stay not-cancelled"
        );

        let master_ev = out
            .iter()
            .find(|e| e.recurrence.is_some())
            .expect("master emitted");
        assert!(
            !master_ev.cancelled,
            "the series master stays not-cancelled"
        );
    }
}
