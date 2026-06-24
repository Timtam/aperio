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

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info};

use cal_core::{CalendarFeature, ContactsFeature, DateRange, TasksFeature};

use super::{
    swr, CacheObserver, CacheRefreshStatus, CacheStore, CacheUpdatedPayload, RefreshCoordinator,
    SyncScope,
};
use crate::db::SharedConn;
use crate::registry::AdapterRegistry;
use crate::user_prefs::UserPrefsRepo;

/// Rolling event window the warm pass preloads, relative to "now".
const WINDOW_PAST_DAYS: i64 = 92; // ~3 months back
const WINDOW_FUTURE_DAYS: i64 = 366; // ~12 months ahead

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
    /// `true` while a pass runs; concurrent triggers no-op.
    in_flight: Arc<Mutex<bool>>,
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
            worker.warm_all().await;

            loop {
                let minutes = worker.read_interval_minutes();
                let dur = StdDuration::from_secs(u64::from(minutes) * 60);
                tokio::select! {
                    _ = tokio::time::sleep(dur) => {
                        debug!(target: "aperio::cache", ?dur, "periodic cache warm tick");
                        worker.warm_all().await;
                    }
                    _ = worker.notify.notified() => {
                        debug!(target: "aperio::cache", "manual cache warm trigger");
                        worker.warm_all().await;
                    }
                }
            }
        });
    }

    /// Wake the worker for an immediate pass (manual refresh / settings
    /// change). No-op if a pass is already running.
    pub fn trigger(&self) {
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
            // point-in-time query carries no target counts.
            total_targets: None,
            fetched_targets: None,
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
    pub async fn warm_all(self: &Arc<Self>) {
        // Single-flight: a periodic tick that lands while a manual pass
        // is still running just bails.
        {
            let mut guard = self.in_flight.lock().expect("cache refresher poisoned");
            if *guard {
                return;
            }
            *guard = true;
        }
        let last = self.status().last_refreshed_at;
        // Spinner on immediately; the target total isn't known until the cheap
        // enumeration below completes.
        self.emit_status(true, last.clone(), None, None);

        let now = Utc::now();
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
        self.emit_status(true, last.clone(), Some(total), Some(0));

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
                me.emit_status(true, last, Some(total), Some(done));
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
        *self.in_flight.lock().expect("cache refresher poisoned") = false;
        self.emit_status(
            false,
            Some(completed.to_rfc3339()),
            Some(total),
            Some(total),
        );
        debug!(target: "aperio::cache", "cache warm pass complete");
    }

    async fn enumerate_calendars(&self, out: &mut Vec<RefreshTarget>) {
        for (account, adapter) in self.registry.snapshot_calendar_adapters() {
            match adapter.list_calendars().await {
                Ok(cals) => {
                    for c in &cals {
                        self.registry.note_calendar_route(&c.id, &account);
                    }
                    let _ = self.cache.replace_calendars(&account, &cals);
                    self.emit_updated(SyncScope::Calendars, &account, "");
                    for cal in cals {
                        out.push(RefreshTarget::Events {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            cal_id: cal.id,
                        });
                    }
                }
                Err(err) => {
                    let _ =
                        self.cache
                            .mark_error(&account, SyncScope::Calendars, "", &err.to_string());
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
                    let _ = self.cache.replace_task_lists(&account, &lists);
                    self.emit_updated(SyncScope::TaskLists, &account, "");
                    for list in lists {
                        out.push(RefreshTarget::Tasks {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            list_id: list.id,
                        });
                    }
                }
                Err(err) => {
                    let _ =
                        self.cache
                            .mark_error(&account, SyncScope::TaskLists, "", &err.to_string());
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
                    let _ = self.cache.replace_contact_lists(&account, &lists);
                    self.emit_updated(SyncScope::ContactLists, &account, "");
                    for list in lists {
                        out.push(RefreshTarget::Contacts {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            list_id: list.id,
                        });
                    }
                }
                Err(err) => {
                    let _ = self.cache.mark_error(
                        &account,
                        SyncScope::ContactLists,
                        "",
                        &err.to_string(),
                    );
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
                if !self.coord.try_claim(&key) {
                    return;
                }
                match swr::refresh_events(&self.cache, adapter.as_ref(), &account, &cal_id, window)
                    .await
                {
                    Ok(()) => self.emit_updated(SyncScope::Events, &account, &cal_id),
                    Err(err) => {
                        let _ = self.cache.mark_error(
                            &account,
                            SyncScope::Events,
                            &cal_id,
                            &err.to_string(),
                        );
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
                if !self.coord.try_claim(&key) {
                    return;
                }
                match swr::refresh_tasks(&self.cache, adapter.as_ref(), &account, &list_id).await {
                    Ok(()) => self.emit_updated(SyncScope::Tasks, &account, &list_id),
                    Err(err) => {
                        let _ = self.cache.mark_error(
                            &account,
                            SyncScope::Tasks,
                            &list_id,
                            &err.to_string(),
                        );
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
                if !self.coord.try_claim(&key) {
                    return;
                }
                match swr::refresh_contacts(&self.cache, adapter.as_ref(), &account, &list_id).await
                {
                    Ok(()) => self.emit_updated(SyncScope::Contacts, &account, &list_id),
                    Err(err) => {
                        let _ = self.cache.mark_error(
                            &account,
                            SyncScope::Contacts,
                            &list_id,
                            &err.to_string(),
                        );
                    }
                }
                self.coord.release(&key);
            }
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
    ) {
        self.observer.refresh_status(&CacheRefreshStatus {
            refreshing,
            last_refreshed_at,
            total_targets,
            fetched_targets,
        });
    }
}
