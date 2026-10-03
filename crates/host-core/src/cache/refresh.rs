//! Background warm + periodic refresh of the external-adapter snapshot
//! cache (CACHE-3).
//!
//! CACHE-1/2 made every external read stale-while-revalidate: serve the
//! snapshot, refresh the touched container in the background. This module
//! adds the proactive half:
//!
//!   - **Startup warm** — shortly after boot, pull every external
//!     account's containers + a WIDE event window (−3…+12 months) into
//!     the cache, so the *next* app start paints instantly and in-window
//!     month/week/day navigation is a cache hit instead of a cold fetch.
//!   - **Periodic refresh** — repeat on a prefs-driven interval so an
//!     open app stays current without the user touching anything.
//!   - **Manual refresh** — `trigger` kicks an immediate pass.
//!
//! Every container write notifies the host via
//! [`CacheObserver::cache_updated`] (the frontend invalidates the
//! matching view); pass start/end go through
//! [`CacheObserver::refresh_status`] so the toolbar can show a spinner +
//! "last updated". Refreshes are deduplicated against the per-read SWR
//! path via the shared [`RefreshCoordinator`].
//!
//! Construction ([`CacheRefresher::new`]) is deliberately split from the
//! scheduler ([`CacheRefresher::start_periodic`]): a host that wants
//! manual-only refresh (mobile) can build the refresher without ever
//! starting the background loop.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Timelike, Utc};
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use cal_core::{CalendarFeature, ContactsFeature, DateRange, TasksFeature};

use super::{
    swr, CacheObserver, CacheRefreshStatus, CacheStore, CacheUpdatedPayload, FailingAccount,
    PassOutcome, RefreshCoordinator, SyncScope,
};
use crate::db::SharedConn;
use crate::registry::AdapterRegistry;
use crate::user_prefs::UserPrefsRepo;

/// Rolling event window the warm pass preloads, relative to "now".
/// `pub(crate)` because the SWR full-resync path widens its fetch to this
/// same window (see [`swr::refresh_events`]) so a view-sized full fetch
/// can't clobber the warm cache down to a month.
pub(crate) const WINDOW_PAST_DAYS: i64 = 92; // ~3 months back
pub(crate) const WINDOW_FUTURE_DAYS: i64 = 366; // ~12 months ahead

/// Let the UI settle before the first (network-heavy, write-heavy) warm
/// pass. The first view already serves from the persisted cache, so there
/// is no rush — and the read pool means a view never blocks on the warm
/// pass's writes anyway. Delaying past the plugin-load + sync-adapter
/// restore burst keeps the user's first interactions on a clear runway.
const APP_START_DELAY: StdDuration = StdDuration::from_secs(8);

const DEFAULT_INTERVAL_MINUTES: u32 = 30;

/// Max container refreshes in flight during a warm pass — overlaps a slow account
/// with the rest without hammering any single provider (≈ a browser's per-host
/// connection budget).
const REFRESH_CONCURRENCY: usize = 6;

/// `user_prefs` keys.
pub const PREF_CACHE_REFRESH_INTERVAL_MINUTES: &str = "cache.refreshIntervalMinutes";
pub const PREF_CACHE_LAST_REFRESHED_AT: &str = "cache.lastRefreshedAt";

pub struct CacheRefresher {
    registry: Arc<AdapterRegistry>,
    cache: Arc<CacheStore>,
    coord: Arc<RefreshCoordinator>,
    db: SharedConn,
    /// Where refresh progress is surfaced (Tauri events on desktop, the
    /// FFI bridge on mobile).
    observer: Arc<dyn CacheObserver>,
    /// Wakes the periodic loop for a manual / settings-driven pass.
    notify: Arc<Notify>,
    /// `true` while a pass runs, and across the queued follow-ups after it.
    /// A plain [`Self::warm_all`] while it is set does nothing; a
    /// [`Self::warm_all_queued`] records one follow-up instead.
    in_flight: Arc<Mutex<bool>>,
    /// Sticky "a USER asked for the next pass" latch. `Notify` holds at
    /// most ONE permit, so a user `trigger` and an automatic
    /// `trigger_background` arriving before the worker wakes collapse into
    /// a single pass — hence a latch that only `trigger` SETS and the pass
    /// CONSUMES, never a value the later caller overwrites. Collapsing
    /// therefore resolves in the user's favour (forced), and forced-ness
    /// can never leak into a later, unrelated wake.
    next_trigger_forced: Arc<AtomicBool>,
    /// Whether the CURRENT pass was user-forced (manual refresh). Set at
    /// the start of every pass, a queued follow-up included; read by the
    /// enumerate/refresh failure paths
    /// so a forced failure surfaces at once (see `CacheStore::mark_error`).
    /// Single-flight makes this stable for a pass's duration; the
    /// concurrent per-read SWR path never touches it.
    pass_forced: Arc<AtomicBool>,
    /// A pass asked for through [`Self::warm_all_queued`] while another ran:
    /// `Some(forced)` runs once more when the running pass ends. Bounded to
    /// one: any number of requests during a pass collapse into a single
    /// follow-up, forced if any of them was.
    follow_up: Arc<Mutex<Option<bool>>>,
    /// Last successful pass, kept in memory + mirrored to prefs.
    last_refreshed: Arc<Mutex<Option<DateTime<Utc>>>>,
}

/// One container queued for an items refresh in a warm pass. Collected during a
/// cheap enumeration pass so the TOTAL is known up front (for "fetched X of N"
/// progress) and the set can be refreshed concurrently.
enum RefreshTarget {
    Events {
        account: String,
        adapter: Arc<dyn CalendarFeature>,
        cal_id: String,
    },
    Tasks {
        account: String,
        adapter: Arc<dyn TasksFeature>,
        list_id: String,
    },
    /// A task list's SECTIONS. Shares the list's `TasksFeature` adapter
    /// (sections are enumerated via `list_sections` on the same trait), so
    /// it's queued right beside the list's `Tasks` target.
    Sections {
        account: String,
        adapter: Arc<dyn TasksFeature>,
        list_id: String,
    },
    Contacts {
        account: String,
        adapter: Arc<dyn ContactsFeature>,
        list_id: String,
    },
}

impl CacheRefresher {
    /// Construct the refresher WITHOUT starting any background worker.
    /// The returned `Arc` is shared with the host so a manual-refresh
    /// command can drive a pass through the same in-flight guard. Call
    /// [`Self::start_periodic`] to enable the warm-on-boot + periodic
    /// loop (desktop); a manual-only host can skip it.
    pub fn new(
        registry: Arc<AdapterRegistry>,
        cache: Arc<CacheStore>,
        coord: Arc<RefreshCoordinator>,
        db: SharedConn,
        observer: Arc<dyn CacheObserver>,
    ) -> Arc<Self> {
        let initial_last = {
            let repo = UserPrefsRepo::new(&db);
            repo.get(PREF_CACHE_LAST_REFRESHED_AT)
                .ok()
                .flatten()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc))
        };

        Arc::new(Self {
            registry,
            cache,
            coord,
            db,
            observer,
            notify: Arc::new(Notify::new()),
            in_flight: Arc::new(Mutex::new(false)),
            pass_forced: Arc::new(AtomicBool::new(false)),
            follow_up: Arc::new(Mutex::new(None)),
            // Unset until a user `trigger` latches it.
            next_trigger_forced: Arc::new(AtomicBool::new(false)),
            last_refreshed: Arc::new(Mutex::new(initial_last)),
        })
    }

    /// Start the background worker: a warm pass after [`APP_START_DELAY`],
    /// then a `tokio::select!` loop that re-warms on the prefs-driven
    /// interval or whenever [`Self::trigger`] fires. Spawned onto `rt` so
    /// it never blocks a command.
    pub fn start_periodic(self: &Arc<Self>, rt: &tokio::runtime::Handle) {
        let worker = self.clone();
        rt.spawn(async move {
            // Let the UI settle before the first (network-heavy) warm pass — but a
            // manual trigger short-circuits the wait, so a cache-generation reset
            // (or the user hitting "Re-sync from scratch") in these first seconds
            // re-fetches AT ONCE instead of after the full delay.
            tokio::select! {
                _ = tokio::time::sleep(APP_START_DELAY) => {}
                _ = worker.notify.notified() => {}
            }
            info!(target: "aperio::cache", "running app-start cache warm pass");
            // The app-start pass is NOT forced on its own: it is the one most
            // prone to a network-not-ready blip, so its failures must be
            // confirmed by a second attempt before they surface. Unless a
            // USER `trigger` cut the delay short (a click on "refresh now" in
            // the first seconds): THIS pass then serves it, and runs forced,
            // so the sentence that ends it names a failure at once, as a
            // manual refresh promises. CONSUME the latch either way, so it
            // cannot force an unrelated later pass. Automatic wakes
            // (`trigger_background`, the cache-generation reset at launch)
            // never set it.
            let forced = worker.next_trigger_forced.swap(false, Ordering::Relaxed);
            worker.warm_all(forced).await;

            loop {
                let minutes = worker.read_interval_minutes();
                let dur = StdDuration::from_secs(u64::from(minutes) * 60);
                tokio::select! {
                    _ = tokio::time::sleep(dur) => {
                        debug!(target: "aperio::cache", ?dur, "periodic cache warm tick");
                        worker.warm_all(false).await;
                    }
                    _ = worker.notify.notified() => {
                        // Forced only when the wake came from a USER action
                        // (`trigger`); an automatic wake (`trigger_background`,
                        // e.g. accounts that just arrived over sync) still
                        // needs its failures confirmed.
                        // CONSUME the latch: a collapsed forced+background
                        // pair resolves as forced, and the flag can't leak
                        // into the next automatic wake.
                        let forced = worker.next_trigger_forced.swap(false, Ordering::Relaxed);
                        debug!(target: "aperio::cache", forced, "cache warm trigger");
                        worker.warm_all(forced).await;
                    }
                }
            }
        });
    }

    /// Wake the worker for an immediate pass on an explicit USER action
    /// (manual refresh, settings/account change). No-op if a pass is
    /// already running. The pass runs FORCED, so its failures surface at
    /// once instead of waiting for a confirming second attempt.
    pub fn trigger(&self) {
        self.next_trigger_forced.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// Wake the worker for an immediate pass that is NOT a user action —
    /// e.g. accounts that just arrived over sync and need their first
    /// listing. Runs UN-forced: a network blip during it must still be
    /// confirmed by a second attempt before it surfaces, exactly like the
    /// app-start / periodic pass (auth failures surface immediately
    /// either way).
    pub fn trigger_background(&self) {
        // Deliberately does NOT touch the latch: it must never downgrade a
        // user request that is already waiting for the same wake.
        self.notify.notify_one();
    }

    pub fn status(&self) -> CacheRefreshStatus {
        CacheRefreshStatus {
            refreshing: *self.in_flight.lock().expect("cache refresher poisoned"),
            last_refreshed_at: self
                .last_refreshed
                .lock()
                .expect("cache refresher poisoned")
                .map(|d| d.to_rfc3339()),
            // Live progress rides the refresh_status STREAM during a pass; a
            // point-in-time query carries no target counts, and no outcome
            // (the error surface is the lasting record of what failed).
            total_targets: None,
            fetched_targets: None,
            outcome: None,
        }
    }

    fn read_interval_minutes(&self) -> u32 {
        let repo = UserPrefsRepo::new(&self.db);
        repo.get(PREF_CACHE_REFRESH_INTERVAL_MINUTES)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|m| m.max(1))
            .unwrap_or(DEFAULT_INTERVAL_MINUTES)
    }

    /// One full warm pass over every external account: enumerate the containers,
    /// then refresh their items with bounded concurrency so a slow provider can't
    /// gate the rest. Dedup-guarded; runs on the background runtime so it never
    /// blocks a command.
    /// `forced` = the pass was requested by an explicit user action
    /// (manual refresh, post-account-change trigger) rather than the
    /// app-start / periodic schedule. A forced pass's failures surface on
    /// the error screen at once instead of waiting for a confirming second
    /// attempt (see `CacheStore::mark_error`).
    pub async fn warm_all(self: &Arc<Self>, forced: bool) {
        // Single-flight: a periodic tick that lands while a manual pass
        // is still running just bails.
        {
            let mut guard = self.in_flight.lock().expect("cache refresher poisoned");
            if *guard {
                return;
            }
            *guard = true;
        }
        self.run_passes(forced).await;
    }

    /// [`Self::warm_all`] for a request that must not be lost.
    ///
    /// `warm_all` drops a request that arrives while a pass runs, which is
    /// right for a periodic tick and wrong for "the data just changed": that
    /// request is about data the running pass may already have read past. On
    /// the desktop the worker's `Notify` keeps such a wake; the phone has no
    /// worker, so a manual refresh, an account reset or a fresh OS grant
    /// that landed during the launch pass simply never happened. Here the
    /// request waits, and the running pass runs once more when it ends.
    pub async fn warm_all_queued(self: &Arc<Self>, forced: bool) {
        {
            let mut guard = self.in_flight.lock().expect("cache refresher poisoned");
            if *guard {
                let mut follow_up = self.follow_up.lock().expect("cache refresher poisoned");
                *follow_up = Some(follow_up.unwrap_or(false) || forced);
                return;
            }
            *guard = true;
        }
        self.run_passes(forced).await;
    }

    /// Run passes while one is queued behind the current one. `in_flight`
    /// stays set across them, and the indicator hears "finished" once.
    async fn run_passes(self: &Arc<Self>, mut forced: bool) {
        loop {
            let (completed, total) = self.pass(forced).await;
            // Before the lock below: a read here must not widen the gap
            // between clearing `in_flight` and saying "finished".
            let outcome = self.pass_outcome();
            // Under the same lock that queues a follow-up, so a request is
            // either taken here or finds the flag cleared and runs itself.
            let next = {
                let mut in_flight = self.in_flight.lock().expect("cache refresher poisoned");
                let next = self
                    .follow_up
                    .lock()
                    .expect("cache refresher poisoned")
                    .take();
                if next.is_none() {
                    *in_flight = false;
                }
                next
            };
            match next {
                Some(queued) => {
                    debug!(target: "aperio::cache", forced = queued, "a queued cache warm pass follows");
                    forced = queued;
                }
                None => {
                    self.emit_status(
                        false,
                        Some(completed.to_rfc3339()),
                        Some(total),
                        Some(total),
                        outcome,
                    );
                    debug!(target: "aperio::cache", "cache warm pass complete");
                    return;
                }
            }
        }
    }

    /// One pass; the caller holds `in_flight`.
    async fn pass(self: &Arc<Self>, forced: bool) -> (DateTime<Utc>, u32) {
        // Record the pass's forced-ness for the failure paths. Safe under
        // single-flight: only this pass writes it, and the concurrent SWR
        // path passes its own (false) flag to mark_error directly.
        self.pass_forced.store(forced, Ordering::Relaxed);
        let last = self.status().last_refreshed_at;
        // Spinner on immediately; the target total isn't known until the cheap
        // enumeration below completes.
        self.emit_status(true, last.clone(), None, None, None);

        // Whole-second endpoints: the window crosses the mobile FFI as
        // RFC-3339 strings and the iOS EventKit bridge's ISO-8601 parser
        // rejects fractional seconds (a raw Utc::now() endpoint made every
        // device-calendar warm fetch throw "invalid event range").
        let now = Utc::now().with_nanosecond(0).unwrap_or_else(Utc::now);
        let window = DateRange::new(
            now - Duration::days(WINDOW_PAST_DAYS),
            now + Duration::days(WINDOW_FUTURE_DAYS),
        );

        // Phase 1 — enumerate every container (cheap list calls) + replace the
        // container lists, collecting the per-container items targets. The total
        // is known up front so the indicator can report "fetched X of N".
        let mut targets = Vec::new();
        self.enumerate_calendars(&mut targets).await;
        self.enumerate_task_lists(&mut targets).await;
        self.enumerate_contact_lists(&mut targets).await;
        let total = targets.len() as u32;
        self.emit_status(true, last.clone(), Some(total), Some(0), None);

        // Phase 2 — refresh each target's items with bounded concurrency, so a
        // slow account overlaps the others instead of serialising the whole pass.
        // Progress (fetched X of N) is reported as each target lands; the shared
        // RefreshCoordinator still dedups against any per-read SWR refresh.
        let fetched = Arc::new(AtomicU32::new(0));
        let sem = Arc::new(Semaphore::new(REFRESH_CONCURRENCY));
        let mut set = JoinSet::new();
        for target in targets {
            let me = Arc::clone(self);
            let sem = Arc::clone(&sem);
            let fetched = Arc::clone(&fetched);
            let last = last.clone();
            set.spawn(async move {
                // Bound in-flight refreshes; the permit is held for the fetch.
                let Ok(_permit) = sem.acquire_owned().await else {
                    return;
                };
                me.refresh_one(target, window).await;
                let done = fetched.fetch_add(1, Ordering::Relaxed) + 1;
                me.emit_status(true, last, Some(total), Some(done), None);
            });
        }
        while set.join_next().await.is_some() {}

        // Stamp completion (in memory + prefs so the indicator survives a
        // restart with a meaningful "last updated").
        let completed = Utc::now();
        *self
            .last_refreshed
            .lock()
            .expect("cache refresher poisoned") = Some(completed);
        let repo = UserPrefsRepo::new(&self.db);
        let _ = repo.set(PREF_CACHE_LAST_REFRESHED_AT, &completed.to_rfc3339());
        (completed, total)
    }

    async fn enumerate_calendars(&self, out: &mut Vec<RefreshTarget>) {
        for (account, adapter) in self.registry.snapshot_calendar_adapters() {
            match adapter.list_calendars().await {
                Ok(cals) => {
                    for c in &cals {
                        self.registry.note_calendar_route(&c.id, &account);
                    }
                    // Only notify when the listing actually changed — a
                    // no-op warm pass must not trigger frontend reloads.
                    if self
                        .cache
                        .replace_calendars(&account, &cals)
                        .unwrap_or(true)
                    {
                        self.emit_updated(SyncScope::Calendars, &account, "");
                    }
                    for cal in cals {
                        out.push(RefreshTarget::Events {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            cal_id: cal.id,
                        });
                    }
                }
                Err(err) => {
                    self.failed(&account, SyncScope::Calendars, "", &err);
                }
            }
        }
    }

    async fn enumerate_task_lists(&self, out: &mut Vec<RefreshTarget>) {
        for (account, adapter) in self.registry.snapshot_task_adapters() {
            match adapter.list_task_lists().await {
                Ok(lists) => {
                    for l in &lists {
                        self.registry.note_task_list_route(&l.id, &account);
                    }
                    if self
                        .cache
                        .replace_task_lists(&account, &lists)
                        .unwrap_or(true)
                    {
                        self.emit_updated(SyncScope::TaskLists, &account, "");
                    }
                    for list in lists {
                        // Warm the list's tasks AND its sections — both ride the
                        // same TasksFeature adapter, so a section warm costs one
                        // extra `list_sections` per external list.
                        out.push(RefreshTarget::Tasks {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            list_id: list.id.clone(),
                        });
                        out.push(RefreshTarget::Sections {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            list_id: list.id,
                        });
                    }
                }
                Err(err) => {
                    self.failed(&account, SyncScope::TaskLists, "", &err);
                }
            }
        }
    }

    async fn enumerate_contact_lists(&self, out: &mut Vec<RefreshTarget>) {
        for (account, adapter) in self.registry.snapshot_contact_adapters() {
            match adapter.list_contact_lists().await {
                Ok(lists) => {
                    for l in &lists {
                        self.registry.note_contact_list_route(&l.id, &account);
                    }
                    if self
                        .cache
                        .replace_contact_lists(&account, &lists)
                        .unwrap_or(true)
                    {
                        self.emit_updated(SyncScope::ContactLists, &account, "");
                    }
                    for list in lists {
                        out.push(RefreshTarget::Contacts {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            list_id: list.id,
                        });
                    }
                }
                Err(err) => {
                    self.failed(&account, SyncScope::ContactLists, "", &err);
                }
            }
        }
    }

    /// Refresh one container's items, deduped against the per-read SWR path via
    /// the shared coordinator. A claim miss means a per-read refresh is already
    /// handling it — skip without releasing (we never claimed).
    async fn refresh_one(&self, target: RefreshTarget, window: DateRange) {
        match target {
            RefreshTarget::Events {
                account,
                adapter,
                cal_id,
            } => {
                let key = format!("events:{account}:{cal_id}");
                // Generation-aware: a refresh started before the container
                // changed cannot answer for the change (see `try_claim`).
                let generation =
                    self.cache
                        .refresh_generation(&account, SyncScope::Events, &cal_id);
                if !self.coord.try_claim(&key, generation) {
                    return;
                }
                match swr::refresh_events(&self.cache, adapter.as_ref(), &account, &cal_id, window)
                    .await
                {
                    // `false` = content identical — skip the notification so a
                    // no-op warm pass stays UI-silent.
                    Ok(changed) => {
                        if changed {
                            self.emit_updated(SyncScope::Events, &account, &cal_id);
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Events, &cal_id, &err);
                    }
                }
                self.coord.release(&key);
            }
            RefreshTarget::Tasks {
                account,
                adapter,
                list_id,
            } => {
                let key = format!("tasks:{account}:{list_id}");
                // Generation-aware: a refresh started before the container
                // changed cannot answer for the change (see `try_claim`).
                let generation =
                    self.cache
                        .refresh_generation(&account, SyncScope::Tasks, &list_id);
                if !self.coord.try_claim(&key, generation) {
                    return;
                }
                match swr::refresh_tasks(&self.cache, adapter.as_ref(), &account, &list_id).await {
                    Ok(changed) => {
                        if changed {
                            self.emit_updated(SyncScope::Tasks, &account, &list_id);
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Tasks, &list_id, &err);
                    }
                }
                self.coord.release(&key);
            }
            RefreshTarget::Sections {
                account,
                adapter,
                list_id,
            } => {
                let key = format!("sections:{account}:{list_id}");
                // Generation-aware: a refresh started before the container
                // changed cannot answer for the change (see `try_claim`).
                let generation =
                    self.cache
                        .refresh_generation(&account, SyncScope::Sections, &list_id);
                if !self.coord.try_claim(&key, generation) {
                    return;
                }
                match swr::refresh_sections(&self.cache, adapter.as_ref(), &account, &list_id).await
                {
                    Ok(changed) => {
                        if changed {
                            self.emit_updated(SyncScope::Sections, &account, &list_id);
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Sections, &list_id, &err);
                    }
                }
                self.coord.release(&key);
            }
            RefreshTarget::Contacts {
                account,
                adapter,
                list_id,
            } => {
                let key = format!("contacts:{account}:{list_id}");
                // Generation-aware: a refresh started before the container
                // changed cannot answer for the change (see `try_claim`).
                let generation =
                    self.cache
                        .refresh_generation(&account, SyncScope::Contacts, &list_id);
                if !self.coord.try_claim(&key, generation) {
                    return;
                }
                match swr::refresh_contacts(&self.cache, adapter.as_ref(), &account, &list_id).await
                {
                    Ok(changed) => {
                        if changed {
                            self.emit_updated(SyncScope::Contacts, &account, &list_id);
                            // A contacts change can also change the CALENDAR
                            // LISTING: §10.3 birthday calendars are synthesised
                            // per contact list that has at least one contact
                            // with a birthday, and for external accounts
                            // `list_birthday_calendars` reads the CACHE only.
                            // Without this the freshly-cached contacts never
                            // reach the calendar list — the birthday calendar
                            // stayed missing for the whole session and only
                            // appeared one app start LATER (when the startup
                            // listing finally saw the cached contacts).
                            // Account-wide signal, so no container id.
                            self.emit_updated(SyncScope::Calendars, &account, "");
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Contacts, &list_id, &err);
                    }
                }
                self.coord.release(&key);
            }
        }
    }

    /// Record a failed attempt of this pass, and log it when it is news:
    /// the container was not failing, or fails differently now. An account
    /// that keeps failing the same way says so once, not on every pass.
    fn failed(&self, account: &str, scope: SyncScope, container: &str, err: &cal_core::Error) {
        let forced = self.pass_forced.load(Ordering::Relaxed);
        let news = self
            .cache
            .mark_failure(account, scope, container, err, forced)
            .unwrap_or(true);
        if news {
            tracing::warn!(
                target: "aperio::cache",
                scope = scope.as_str(),
                account,
                container,
                %err,
                "refresh failed",
            );
        }
    }

    fn emit_updated(&self, scope: SyncScope, account: &str, container: &str) {
        self.observer.cache_updated(&CacheUpdatedPayload {
            scope: scope.as_str().to_string(),
            account_id: account.to_string(),
            container_id: container.to_string(),
        });
    }

    fn emit_status(
        &self,
        refreshing: bool,
        last_refreshed_at: Option<String>,
        total_targets: Option<u32>,
        fetched_targets: Option<u32>,
        outcome: Option<PassOutcome>,
    ) {
        self.observer.refresh_status(&CacheRefreshStatus {
            refreshing,
            last_refreshed_at,
            total_targets,
            fetched_targets,
            outcome,
        });
    }

    /// What the pass left undone, named for the sentence that ends it: the
    /// accounts the error surface shows ([`CacheStore::refresh_errors`],
    /// so a first network blip of an unforced pass is not named before it
    /// is confirmed), and whether every account the pass tried failed.
    /// `None` when either could not be read; the surfaces then say their
    /// plain sentence.
    fn pass_outcome(&self) -> Option<PassOutcome> {
        let errors = self
            .cache
            .refresh_errors()
            .map_err(|e| warn!(target: "aperio::cache", error = %e, "pass outcome: refresh errors unreadable"))
            .ok()?;
        let names = self
            .cache
            .account_names()
            .map_err(|e| warn!(target: "aperio::cache", error = %e, "pass outcome: account names unreadable"))
            .ok()?;
        // Dropped: an account deleted while the pass ran.
        let failing: Vec<FailingAccount> = errors
            .iter()
            .filter_map(|acc| {
                names.get(&acc.account_id).map(|name| FailingAccount {
                    account_id: acc.account_id.clone(),
                    name: name.clone(),
                    cause: acc.cause,
                    rank: acc.rank,
                })
            })
            .collect();
        let attempted: std::collections::HashSet<String> = self
            .registry
            .snapshot_calendar_adapters()
            .into_iter()
            .map(|(account, _)| account)
            .chain(
                self.registry
                    .snapshot_task_adapters()
                    .into_iter()
                    .map(|(a, _)| a),
            )
            .chain(
                self.registry
                    .snapshot_contact_adapters()
                    .into_iter()
                    .map(|(a, _)| a),
            )
            .collect();
        let all_failed = !attempted.is_empty()
            && attempted
                .iter()
                .all(|id| failing.iter().any(|f| &f.account_id == id));
        Some(PassOutcome {
            failing,
            all_failed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheUpdatedPayload;
    use crate::db::DbHandle;
    use async_trait::async_trait;
    use cal_core::{
        Adapter, AuthToken, Calendar, Capability, Credentials, Event, FreeBusy, NewEvent,
    };

    /// A calendar account whose FIRST listing waits until the test lets it
    /// go, so a second request can arrive while a pass is running.
    struct Gated {
        listings: AtomicU32,
        entered: Notify,
        release: Notify,
    }

    #[async_trait]
    impl Adapter for Gated {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl CalendarFeature for Gated {
        async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
            if self.listings.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(Vec::new())
        }
        async fn get_events(&self, _: &str, _: DateRange) -> cal_core::Result<Vec<Event>> {
            Ok(Vec::new())
        }
        async fn create_event(&self, _: &str, _: NewEvent) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn update_event(&self, _: Event) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn delete_event(&self, _: &str, _: bool) -> cal_core::Result<()> {
            unreachable!()
        }
        async fn get_free_busy(&self, _: &[&str], _: DateRange) -> cal_core::Result<Vec<FreeBusy>> {
            unreachable!()
        }
        fn calendar_color(&self, _: &str) -> Option<cal_core::ContainerColor> {
            None
        }
    }

    /// Every status the indicator was told, in order.
    #[derive(Default)]
    struct Statuses(Mutex<Vec<CacheRefreshStatus>>);

    impl CacheObserver for Statuses {
        fn cache_updated(&self, _: &CacheUpdatedPayload) {}
        fn refresh_status(&self, status: &CacheRefreshStatus) {
            self.0.lock().unwrap().push(status.clone());
        }
    }

    impl Statuses {
        /// The outcome on the status that ended the last passes.
        fn last_outcome(&self) -> PassOutcome {
            let all = self.0.lock().unwrap();
            let last = all.last().expect("a status was emitted");
            assert!(!last.refreshing, "the passes ended");
            last.outcome.clone().expect("the end carries an outcome")
        }
    }

    fn refresher() -> (Arc<CacheRefresher>, Arc<Gated>, Arc<Statuses>) {
        let db = DbHandle::open_in_memory().unwrap();
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('acc-1', 'caldav', 'Work', '{}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
        })
        .unwrap();
        let registry = Arc::new(AdapterRegistry::new(
            Arc::new(plugin_core::PluginManager::new("0.1.0")),
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
        ));
        let gated = Arc::new(Gated {
            listings: AtomicU32::new(0),
            entered: Notify::new(),
            release: Notify::new(),
        });
        registry.register_host_adapter(
            "acc-1",
            Some(Arc::clone(&gated) as Arc<dyn CalendarFeature>),
            None,
        );
        let statuses = Arc::new(Statuses::default());
        let refresher = CacheRefresher::new(
            registry,
            Arc::new(CacheStore::new(db.clone())),
            Arc::new(RefreshCoordinator::new()),
            db.shared(),
            Arc::clone(&statuses) as Arc<dyn CacheObserver>,
        );
        (refresher, gated, statuses)
    }

    /// A calendar account the OS withholds until `granted` is set.
    struct Withheld {
        listings: AtomicU32,
        granted: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl Adapter for Withheld {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl CalendarFeature for Withheld {
        async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
            self.listings.fetch_add(1, Ordering::SeqCst);
            if self.granted.load(Ordering::SeqCst) {
                Ok(Vec::new())
            } else {
                Err(cal_core::Error::AccessNotGranted(
                    "calendars: Denied".into(),
                ))
            }
        }
        async fn get_events(&self, _: &str, _: DateRange) -> cal_core::Result<Vec<Event>> {
            Ok(Vec::new())
        }
        async fn create_event(&self, _: &str, _: NewEvent) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn update_event(&self, _: Event) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn delete_event(&self, _: &str, _: bool) -> cal_core::Result<()> {
            unreachable!()
        }
        async fn get_free_busy(&self, _: &[&str], _: DateRange) -> cal_core::Result<Vec<FreeBusy>> {
            unreachable!()
        }
        fn calendar_color(&self, _: &str) -> Option<cal_core::ContainerColor> {
            None
        }
    }

    #[tokio::test]
    async fn the_warm_pass_still_asks_a_withheld_account_and_a_grant_lifts_the_block() {
        let (refresher, _gated, _) = refresher();
        let withheld = Arc::new(Withheld {
            listings: AtomicU32::new(0),
            granted: std::sync::atomic::AtomicBool::new(false),
        });
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(Arc::clone(&withheld) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.warm_all(false).await;
        assert!(refresher.cache.access_withheld("acc-1", SyncScope::Events));
        withheld.granted.store(true, Ordering::SeqCst);
        refresher.warm_all(false).await;
        assert_eq!(
            withheld.listings.load(Ordering::SeqCst),
            2,
            "the pass asked again"
        );
        assert!(!refresher.cache.access_withheld("acc-1", SyncScope::Events));
    }

    /// A calendar account whose listing always fails with a network error.
    struct Down;

    #[async_trait]
    impl Adapter for Down {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl CalendarFeature for Down {
        async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
            Err(cal_core::Error::Network("offline".into()))
        }
        async fn get_events(&self, _: &str, _: DateRange) -> cal_core::Result<Vec<Event>> {
            unreachable!()
        }
        async fn create_event(&self, _: &str, _: NewEvent) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn update_event(&self, _: Event) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn delete_event(&self, _: &str, _: bool) -> cal_core::Result<()> {
            unreachable!()
        }
        async fn get_free_busy(&self, _: &[&str], _: DateRange) -> cal_core::Result<Vec<FreeBusy>> {
            unreachable!()
        }
        fn calendar_color(&self, _: &str) -> Option<cal_core::ContainerColor> {
            None
        }
    }

    fn withheld(granted: bool) -> Arc<Withheld> {
        Arc::new(Withheld {
            listings: AtomicU32::new(0),
            granted: std::sync::atomic::AtomicBool::new(granted),
        })
    }

    #[tokio::test]
    async fn the_end_of_a_pass_names_the_accounts_it_could_not_update() {
        // "Externe Daten aktualisiert, außer: Work." (decision 180).
        let (refresher, _gated, statuses) = refresher();
        let work = withheld(false);
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(Arc::clone(&work) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.warm_all(false).await;
        assert_eq!(
            statuses.last_outcome(),
            PassOutcome {
                failing: vec![FailingAccount {
                    account_id: "acc-1".into(),
                    name: "Work".into(),
                    cause: super::super::RefreshCause::Access,
                    rank: 2,
                }],
                // Work was the only account the pass tried.
                all_failed: true,
            }
        );
        // Only the end carries it.
        assert!(statuses
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.refreshing)
            .all(|s| s.outcome.is_none()));

        work.granted.store(true, Ordering::SeqCst);
        refresher.warm_all(false).await;
        assert_eq!(
            statuses.last_outcome(),
            PassOutcome {
                failing: Vec::new(),
                all_failed: false
            }
        );
    }

    #[tokio::test]
    async fn one_failing_account_among_others_is_not_all_failed() {
        let (refresher, _gated, statuses) = refresher();
        refresher
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('acc-2', 'caldav', 'Home', '{}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(withheld(false) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.registry.register_host_adapter(
            "acc-2",
            Some(withheld(true) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.warm_all(false).await;
        let outcome = statuses.last_outcome();
        assert_eq!(
            outcome
                .failing
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Work"]
        );
        assert!(!outcome.all_failed, "Home was updated");
    }

    #[tokio::test]
    async fn an_unconfirmed_blip_is_not_named_but_a_forced_failure_is() {
        // The sentence names what the error surface shows: an unforced
        // pass's first network failure is not confirmed yet.
        let (refresher, _gated, statuses) = refresher();
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(Arc::new(Down) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.warm_all(false).await;
        assert!(statuses.last_outcome().failing.is_empty());
        refresher.warm_all(true).await;
        let outcome = statuses.last_outcome();
        assert_eq!(outcome.failing.len(), 1);
        assert_eq!(outcome.failing[0].cause, super::super::RefreshCause::Other);
        assert!(outcome.all_failed);
    }

    /// The first pass of the desktop worker, woken by `wake` before the
    /// start delay ran out, against an account whose listing is down.
    async fn first_pass_woken_by(wake: fn(&CacheRefresher)) -> PassOutcome {
        let (refresher, _gated, statuses) = refresher();
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(Arc::new(Down) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.start_periodic(&tokio::runtime::Handle::current());
        wake(&refresher);
        loop {
            if statuses.0.lock().unwrap().iter().any(|s| !s.refreshing) {
                break;
            }
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
        statuses.last_outcome()
    }

    #[tokio::test]
    async fn a_click_in_the_first_seconds_runs_the_first_pass_forced() {
        // The user's "refresh now" cut the start delay short: the pass that
        // serves it says its failure at once, as a manual refresh promises.
        let outcome = first_pass_woken_by(CacheRefresher::trigger).await;
        assert_eq!(outcome.failing.len(), 1);
    }

    #[tokio::test]
    async fn an_automatic_wake_in_the_first_seconds_stays_unforced() {
        // The cache-generation reset at launch: a blip is confirmed first.
        let outcome = first_pass_woken_by(CacheRefresher::trigger_background).await;
        assert!(outcome.failing.is_empty());
    }

    #[test]
    fn a_point_in_time_status_has_no_outcome() {
        let (refresher, _gated, _) = refresher();
        assert!(refresher.status().outcome.is_none());
    }

    #[tokio::test]
    async fn a_queued_request_runs_after_the_running_pass() {
        let (refresher, gated, statuses) = refresher();
        let running = tokio::spawn({
            let refresher = Arc::clone(&refresher);
            async move { refresher.warm_all(false).await }
        });
        gated.entered.notified().await;

        // Two requests while the pass runs: one follow-up, forced if either was.
        refresher.warm_all_queued(true).await;
        refresher.warm_all_queued(false).await;
        gated.release.notify_one();
        running.await.unwrap();

        assert_eq!(
            gated.listings.load(Ordering::SeqCst),
            2,
            "the follow-up listed again"
        );
        assert!(
            refresher.pass_forced.load(Ordering::Relaxed),
            "the follow-up ran forced"
        );
        let finished = statuses
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|s| !s.refreshing)
            .count();
        assert_eq!(finished, 1, "the indicator heard 'finished' once");
        assert!(!refresher.status().refreshing);
    }

    #[tokio::test]
    async fn a_plain_request_during_a_pass_is_still_dropped() {
        let (refresher, gated, _) = refresher();
        let running = tokio::spawn({
            let refresher = Arc::clone(&refresher);
            async move { refresher.warm_all(false).await }
        });
        gated.entered.notified().await;
        refresher.warm_all(true).await;
        gated.release.notify_one();
        running.await.unwrap();
        assert_eq!(gated.listings.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_queued_request_with_nothing_running_runs_at_once() {
        let (refresher, gated, _) = refresher();
        gated.release.notify_one();
        refresher.warm_all_queued(false).await;
        assert_eq!(gated.listings.load(Ordering::SeqCst), 1);
        assert!(!refresher.status().refreshing);
    }
}
