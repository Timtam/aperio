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

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use tokio::sync::Notify;
use tracing::{debug, info};

use cal_core::DateRange;

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
            tokio::time::sleep(APP_START_DELAY).await;
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

    /// One full warm pass over every external account. Sequential +
    /// dedup-guarded; runs on the background runtime so it never blocks a
    /// command.
    pub async fn warm_all(&self) {
        // Single-flight: a periodic tick that lands while a manual pass
        // is still running just bails.
        {
            let mut guard = self.in_flight.lock().expect("cache refresher poisoned");
            if *guard {
                return;
            }
            *guard = true;
        }
        self.emit_status(true, self.status().last_refreshed_at);

        let now = Utc::now();
        let window = DateRange::new(
            now - Duration::days(WINDOW_PAST_DAYS),
            now + Duration::days(WINDOW_FUTURE_DAYS),
        );

        self.warm_calendars(window).await;
        self.warm_task_lists().await;
        self.warm_contact_lists().await;

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
        self.emit_status(false, Some(completed.to_rfc3339()));
        debug!(target: "aperio::cache", "cache warm pass complete");
    }

    async fn warm_calendars(&self, window: DateRange) {
        for (account, adapter) in self.registry.snapshot_calendar_adapters() {
            let cals = match adapter.list_calendars().await {
                Ok(cals) => {
                    for c in &cals {
                        self.registry.note_calendar_route(&c.id, &account);
                    }
                    let _ = self.cache.replace_calendars(&account, &cals);
                    self.emit_updated(SyncScope::Calendars, &account, "");
                    cals
                }
                Err(err) => {
                    let _ =
                        self.cache
                            .mark_error(&account, SyncScope::Calendars, "", &err.to_string());
                    continue;
                }
            };
            for cal in &cals {
                let key = format!("events:{account}:{}", cal.id);
                if !self.coord.try_claim(&key) {
                    continue; // a per-read refresh is already handling it
                }
                match swr::refresh_events(&self.cache, adapter.as_ref(), &account, &cal.id, window)
                    .await
                {
                    Ok(()) => self.emit_updated(SyncScope::Events, &account, &cal.id),
                    Err(err) => {
                        let _ = self.cache.mark_error(
                            &account,
                            SyncScope::Events,
                            &cal.id,
                            &err.to_string(),
                        );
                    }
                }
                self.coord.release(&key);
            }
        }
    }

    async fn warm_task_lists(&self) {
        for (account, adapter) in self.registry.snapshot_task_adapters() {
            let lists = match adapter.list_task_lists().await {
                Ok(lists) => {
                    for l in &lists {
                        self.registry.note_task_list_route(&l.id, &account);
                    }
                    let _ = self.cache.replace_task_lists(&account, &lists);
                    self.emit_updated(SyncScope::TaskLists, &account, "");
                    lists
                }
                Err(err) => {
                    let _ =
                        self.cache
                            .mark_error(&account, SyncScope::TaskLists, "", &err.to_string());
                    continue;
                }
            };
            for list in &lists {
                let key = format!("tasks:{account}:{}", list.id);
                if !self.coord.try_claim(&key) {
                    continue;
                }
                match swr::refresh_tasks(&self.cache, adapter.as_ref(), &account, &list.id).await {
                    Ok(()) => self.emit_updated(SyncScope::Tasks, &account, &list.id),
                    Err(err) => {
                        let _ = self.cache.mark_error(
                            &account,
                            SyncScope::Tasks,
                            &list.id,
                            &err.to_string(),
                        );
                    }
                }
                self.coord.release(&key);
            }
        }
    }

    async fn warm_contact_lists(&self) {
        for (account, adapter) in self.registry.snapshot_contact_adapters() {
            let lists = match adapter.list_contact_lists().await {
                Ok(lists) => {
                    for l in &lists {
                        self.registry.note_contact_list_route(&l.id, &account);
                    }
                    let _ = self.cache.replace_contact_lists(&account, &lists);
                    self.emit_updated(SyncScope::ContactLists, &account, "");
                    lists
                }
                Err(err) => {
                    let _ = self.cache.mark_error(
                        &account,
                        SyncScope::ContactLists,
                        "",
                        &err.to_string(),
                    );
                    continue;
                }
            };
            for list in &lists {
                let key = format!("contacts:{account}:{}", list.id);
                if !self.coord.try_claim(&key) {
                    continue;
                }
                match swr::refresh_contacts(&self.cache, adapter.as_ref(), &account, &list.id).await
                {
                    Ok(()) => self.emit_updated(SyncScope::Contacts, &account, &list.id),
                    Err(err) => {
                        let _ = self.cache.mark_error(
                            &account,
                            SyncScope::Contacts,
                            &list.id,
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

    fn emit_status(&self, refreshing: bool, last_refreshed_at: Option<String>) {
        self.observer.refresh_status(&CacheRefreshStatus {
            refreshing,
            last_refreshed_at,
        });
    }
}
