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
//!   - **Periodic refresh** — repeat on the one refresh interval
//!     (`sync.intervalMinutes`, Einstellungen → Allgemein; Toni,
//!     2026-11-09) so an open app stays current without the user touching
//!     anything. A change of the interval re-times the wait at once
//!     ([`CacheRefresher::interval_changed`]), without an extra pass.
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
    swr, Attempt, CacheObserver, CacheRefreshStatus, CacheStore, CacheUpdatedPayload,
    FailingAccount, PassOutcome, RefreshCoordinator, SyncScope,
};
use crate::contact_sync::{ContactSyncCore, ContactSyncObserver};
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

/// Max container refreshes in flight during a warm pass — overlaps a slow account
/// with the rest without hammering any single provider (≈ a browser's per-host
/// connection budget).
const REFRESH_CONCURRENCY: usize = 6;

/// `user_prefs` key: when the last pass finished.
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
    /// Wakes the periodic loop to measure its wait again, without a pass:
    /// the interval changed ([`Self::interval_changed`]).
    reschedule: Arc<Notify>,
    /// `true` while a pass runs, and across the queued follow-ups after it.
    /// A plain [`Self::warm_all`] while it is set does nothing when the
    /// running pass reads all it asks for ([`PassScope::covers`]); a
    /// [`Self::warm_all_queued`] records one follow-up instead.
    in_flight: Arc<Mutex<bool>>,
    /// Sticky "a USER asked for the next pass" latch. `Notify` holds at
    /// most ONE permit, so a user `trigger` and an automatic
    /// `trigger_background` arriving before the worker wakes collapse into
    /// a single pass — hence a latch that only `trigger` SETS and the pass
    /// CONSUMES, never a value the later caller overwrites. Collapsing
    /// therefore resolves in the user's favour (forced), and forced-ness
    /// can never leak into a later, unrelated wake. Which accounts it
    /// forces ([`Forcing`]): collapsed requests force every account either
    /// forced.
    next_trigger: Arc<Mutex<Forcing>>,
    /// The accounts the CURRENT pass runs forced for (a manual refresh: all;
    /// an account change: that account). Set at the start of every pass, a
    /// queued follow-up included; read by the enumerate/refresh failure
    /// paths so a forced failure surfaces at once (see
    /// `CacheStore::mark_failure`). Single-flight makes this stable for a
    /// pass's duration; the concurrent per-read SWR path never touches it.
    pass_forcing: Arc<Mutex<Forcing>>,
    /// The (account, listing family) pairs a busy provider turned away in the
    /// CURRENT pass (decision 271): the pass asks none of that family's
    /// remaining containers, and records nothing for them. Reset as each pass
    /// begins, so the next one asks again.
    busy_families: Arc<Mutex<std::collections::HashSet<(String, &'static str)>>>,
    /// Whether the running pass read anything successfully: a listing, or a
    /// container. Reset at the start of every pass; "nothing could be
    /// updated" is said only when it stayed false, not because every account
    /// has SOME failing container.
    pass_succeeded: Arc<AtomicBool>,
    /// A pass asked for through [`Self::warm_all_queued`] while another ran:
    /// `Some(forcing)` runs once more when the running pass ends. Bounded to
    /// one: any number of requests during a pass collapse into a single
    /// follow-up, forced for every account any of them forced.
    follow_up: Arc<Mutex<Option<(Forcing, PassScope)>>>,
    /// What the running pass reads. Set under `in_flight`'s lock whenever a
    /// pass is about to start, so a request that finds a pass running can
    /// tell whether that pass stands in for it ([`PassScope::covers`]).
    running_scope: Arc<Mutex<PassScope>>,
    /// Where a pass reports the contacts it read: "last synced" and the
    /// done signal of the contacts sync, which IS this refresh (Toni,
    /// 2026-11-09). Installed by the host ([`Self::set_contact_sync`]).
    contact_sync: Arc<Mutex<Option<ContactSync>>>,
    /// The contact accounts the current pass read, and whether all of each
    /// one's lists came through.
    contact_accounts: Arc<Mutex<std::collections::BTreeMap<String, bool>>>,
    /// Last successful pass, kept in memory + mirrored to prefs.
    last_refreshed: Arc<Mutex<Option<DateTime<Utc>>>>,
}

/// Which accounts a warm pass runs forced for: a failure there shows at once
/// instead of waiting for a confirming second round (decision 271). A manual
/// refresh forces every account; a change of one account forces only that
/// one (Toni, 2026-10-23); the rest count as an automatic round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Forcing {
    #[default]
    None,
    All,
    Accounts(std::collections::BTreeSet<String>),
}

impl Forcing {
    /// Forced for one account.
    pub fn account(id: &str) -> Self {
        Self::Accounts(std::collections::BTreeSet::from([id.to_string()]))
    }

    /// Two requests collapsed into one pass: forced for every account
    /// either forced.
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::All, _) | (_, Self::All) => Self::All,
            (Self::None, other) | (other, Self::None) => other,
            (Self::Accounts(mut one), Self::Accounts(two)) => {
                one.extend(two);
                Self::Accounts(one)
            }
        }
    }

    /// Whether a failure of `account` shows at once.
    pub fn covers(&self, account: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Accounts(accounts) => accounts.contains(account),
        }
    }
}

impl From<bool> for Forcing {
    fn from(forced: bool) -> Self {
        if forced {
            Self::All
        } else {
            Self::None
        }
    }
}

/// What ended the periodic loop's wait ([`CacheRefresher::next_wake`]).
#[derive(Debug, PartialEq, Eq)]
enum Wake {
    Tick,
    Trigger,
    Retime,
}

/// What a pass reads. Every pass reads the contact lists; a contacts-only
/// one ("sync contacts now") reads nothing else. The read-only directories
/// (a GAL, Google's directory, Suggested People) are large and often have no
/// delta: a periodic pass reads them only when the user asked for it
/// (`contacts.includeReadOnlyOnSync`); the app-start pass, a manual refresh
/// and a change of an account read them as ever, and so does opening one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassScope {
    pub only_contacts: bool,
    pub read_only_directories: bool,
}

impl PassScope {
    /// Everything, directories included.
    pub const FULL: Self = Self {
        only_contacts: false,
        read_only_directories: true,
    };

    /// Two requests collapsed into one pass: it reads what either asked for.
    pub fn merge(self, other: Self) -> Self {
        Self {
            only_contacts: self.only_contacts && other.only_contacts,
            read_only_directories: self.read_only_directories || other.read_only_directories,
        }
    }

    /// Whether a pass of this scope reads everything `wanted` asks for.
    pub fn covers(self, wanted: Self) -> bool {
        (!self.only_contacts || wanted.only_contacts)
            && (self.read_only_directories || !wanted.read_only_directories)
    }
}

/// A host's contacts-sync status holder and its done-signal sink.
type ContactSync = (Arc<ContactSyncCore>, Arc<dyn ContactSyncObserver>);

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
            reschedule: Arc::new(Notify::new()),
            in_flight: Arc::new(Mutex::new(false)),
            pass_forcing: Arc::new(Mutex::new(Forcing::None)),
            busy_families: Arc::new(Mutex::new(std::collections::HashSet::new())),
            pass_succeeded: Arc::new(AtomicBool::new(false)),
            follow_up: Arc::new(Mutex::new(None)),
            running_scope: Arc::new(Mutex::new(PassScope::FULL)),
            contact_sync: Arc::new(Mutex::new(None)),
            contact_accounts: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            // Unset until a user `trigger` latches it.
            next_trigger: Arc::new(Mutex::new(Forcing::None)),
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
            let forcing = worker.take_trigger();
            worker.warm_all(forcing).await;

            loop {
                match worker.next_wake().await {
                    Wake::Tick => {
                        debug!(target: "aperio::cache", "periodic cache warm tick");
                        worker.warm_periodic().await;
                    }
                    // The interval changed: measure the wait again.
                    Wake::Retime => continue,
                    Wake::Trigger => {
                        // Forced only when the wake came from a USER action
                        // (`trigger`); an automatic wake (`trigger_background`,
                        // e.g. accounts that just arrived over sync) still
                        // needs its failures confirmed.
                        // CONSUME the latch: a collapsed forced+background
                        // pair resolves as forced, and the flag can't leak
                        // into the next automatic wake.
                        let forcing = worker.take_trigger();
                        debug!(target: "aperio::cache", ?forcing, "cache warm trigger");
                        worker.warm_all(forcing).await;
                    }
                }
            }
        });
    }

    /// What ends the periodic loop's wait: the interval ran out, a pass was
    /// asked for, or the interval changed and the wait is measured again.
    async fn next_wake(&self) -> Wake {
        let interval = crate::user_prefs::sync_interval(&self.db);
        tokio::select! {
            _ = tokio::time::sleep(interval) => Wake::Tick,
            _ = self.notify.notified() => Wake::Trigger,
            _ = self.reschedule.notified() => Wake::Retime,
        }
    }

    /// The refresh interval changed (here, or on another device): the
    /// periodic loop measures its wait again from now, without a pass.
    pub fn interval_changed(&self) {
        self.reschedule.notify_one();
    }

    /// Wake the worker for an immediate pass on the user's manual refresh.
    /// No-op if a pass is already running. The pass runs forced for every
    /// account, so its failures surface at once instead of waiting for a
    /// confirming second attempt. A change of one account goes through
    /// [`Self::trigger_account`], which forces only that one (decision 271).
    pub fn trigger(&self) {
        self.latch(Forcing::All);
        self.notify.notify_one();
    }

    /// [`Self::trigger`] for a change of ONE account (added, edited,
    /// reconnected, reset): the pass runs forced for that account only, and
    /// counts as an automatic round for the others (Toni, 2026-10-23), so an
    /// unchanged account the provider is busy for does not warn after one
    /// round (decision 271).
    pub fn trigger_account(&self, account_id: &str) {
        self.latch(Forcing::account(account_id));
        self.notify.notify_one();
    }

    fn latch(&self, forcing: Forcing) {
        let mut next = self.next_trigger.lock().expect("cache refresher poisoned");
        let before = std::mem::take(&mut *next);
        *next = before.merge(forcing);
    }

    /// CONSUME the latch, so it cannot force an unrelated later pass.
    fn take_trigger(&self) -> Forcing {
        std::mem::take(&mut *self.next_trigger.lock().expect("cache refresher poisoned"))
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

    /// One full warm pass over every external account: enumerate the containers,
    /// then refresh their items with bounded concurrency so a slow provider can't
    /// gate the rest. Dedup-guarded; runs on the background runtime so it never
    /// blocks a command.
    /// `forcing` = which accounts an explicit user action asked for
    /// ([`Forcing`]): every one after a manual refresh, the listed ones after
    /// a change of an account, none on the app-start or periodic schedule.
    /// A forced account's failures surface on the error screen at once
    /// instead of waiting for a confirming second attempt; every other
    /// account counts as an automatic round (see `CacheStore::mark_failure`).
    pub async fn warm_all(self: &Arc<Self>, forcing: impl Into<Forcing>) {
        self.warm_scoped(forcing.into(), PassScope::FULL).await;
    }

    /// A pass on the refresh interval's schedule (the desktop's tick, the
    /// phone's timer and return to the foreground): everything but the
    /// read-only directories, unless the user asked for those too.
    pub async fn warm_periodic(self: &Arc<Self>) {
        self.warm_scoped(Forcing::None, self.periodic_scope()).await;
    }

    /// [`Self::warm_periodic`] for the phone's OS background round, which
    /// must not be lost (see [`Self::warm_all_queued`]).
    pub async fn warm_periodic_queued(self: &Arc<Self>) {
        self.queued(Forcing::None, self.periodic_scope()).await;
    }

    fn periodic_scope(&self) -> PassScope {
        PassScope {
            only_contacts: false,
            read_only_directories: crate::contact_sync::include_read_only_on_sync(&self.db),
        }
    }

    /// "Sync contacts now": the contact lists alone, the read-only
    /// directories with them when `include_read_only` says so. Forced, as a
    /// user action is, and queued behind a running pass, never dropped — the
    /// user pressed a button. `true` when the pass ran within this call,
    /// `false` when it waits behind the running one; its `contacts-synced`
    /// follows either way.
    pub async fn warm_contacts(self: &Arc<Self>, include_read_only: bool) -> bool {
        let scope = PassScope {
            only_contacts: true,
            read_only_directories: include_read_only,
        };
        self.queued(Forcing::All, scope).await
    }

    /// Single-flight: a request that lands while a pass is still running
    /// bails when that pass reads everything it asks for; otherwise (the
    /// running one reads only contacts, or leaves out the directories) the
    /// rest follows it as the queued pass.
    async fn warm_scoped(self: &Arc<Self>, forcing: Forcing, scope: PassScope) {
        {
            let mut guard = self.in_flight.lock().expect("cache refresher poisoned");
            if *guard {
                let running = *self.running_scope.lock().expect("cache refresher poisoned");
                if !running.covers(scope) {
                    self.queue_follow_up(forcing, scope);
                }
                return;
            }
            *guard = true;
            *self.running_scope.lock().expect("cache refresher poisoned") = scope;
        }
        self.run_passes(forcing, scope).await;
    }

    /// Record a pass to run once the running one ends, merged into one
    /// already waiting. The caller holds `in_flight`'s lock.
    fn queue_follow_up(&self, forcing: Forcing, scope: PassScope) {
        let mut follow_up = self.follow_up.lock().expect("cache refresher poisoned");
        *follow_up = Some(match follow_up.take() {
            Some((f, sc)) => (f.merge(forcing), sc.merge(scope)),
            None => (forcing, scope),
        });
    }

    /// Install where a pass reports the contacts it read (see
    /// [`ContactSyncCore`]): each host does, once, after construction.
    pub fn set_contact_sync(
        &self,
        core: Arc<ContactSyncCore>,
        observer: Arc<dyn ContactSyncObserver>,
    ) {
        *self.contact_sync.lock().expect("cache refresher poisoned") = Some((core, observer));
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
    pub async fn warm_all_queued(self: &Arc<Self>, forcing: impl Into<Forcing>) {
        self.queued(forcing.into(), PassScope::FULL).await;
    }

    /// [`Self::warm_all_queued`] for any scope: queued requests collapse
    /// into one follow-up that reads what any of them asked for. `true` when
    /// the pass ran within this call, `false` when it was queued.
    async fn queued(self: &Arc<Self>, forcing: Forcing, scope: PassScope) -> bool {
        {
            let mut guard = self.in_flight.lock().expect("cache refresher poisoned");
            if *guard {
                self.queue_follow_up(forcing, scope);
                return false;
            }
            *guard = true;
            *self.running_scope.lock().expect("cache refresher poisoned") = scope;
        }
        self.run_passes(forcing, scope).await;
        true
    }

    /// Run passes while one is queued behind the current one. `in_flight`
    /// stays set across them, and the indicator hears "finished" once.
    async fn run_passes(self: &Arc<Self>, mut forcing: Forcing, mut scope: PassScope) {
        loop {
            let total = self.pass(forcing, scope).await;
            // Before the lock below: a read here must not widen the gap
            // between clearing `in_flight` and saying "finished".
            let outcome = self.pass_outcome(scope);
            // Under the same lock that queues a follow-up, so a request is
            // either taken here or finds the flag cleared and runs itself.
            let next = {
                let mut in_flight = self.in_flight.lock().expect("cache refresher poisoned");
                let next = self
                    .follow_up
                    .lock()
                    .expect("cache refresher poisoned")
                    .take();
                match &next {
                    Some((_, next_scope)) => {
                        *self.running_scope.lock().expect("cache refresher poisoned") = *next_scope;
                    }
                    None => *in_flight = false,
                }
                next
            };
            match next {
                Some((queued, queued_scope)) => {
                    debug!(target: "aperio::cache", forcing = ?queued, scope = ?queued_scope, "a queued cache warm pass follows");
                    forcing = queued;
                    scope = queued_scope;
                }
                None => {
                    // A contacts-only pass leaves "last updated" as it was.
                    let last = self.status().last_refreshed_at;
                    self.emit_status(false, last, Some(total), Some(total), outcome);
                    debug!(target: "aperio::cache", "cache warm pass complete");
                    return;
                }
            }
        }
    }

    /// One pass; the caller holds `in_flight`.
    async fn pass(self: &Arc<Self>, forcing: Forcing, scope: PassScope) -> u32 {
        self.contact_accounts
            .lock()
            .expect("cache refresher poisoned")
            .clear();
        let contact_sync = self
            .contact_sync
            .lock()
            .expect("cache refresher poisoned")
            .clone();
        if let Some((core, _)) = &contact_sync {
            core.begin_pass();
        }
        // Record the pass's forced-ness for the failure paths. Safe under
        // single-flight: only this pass writes it, and the concurrent SWR
        // path records its own failures through `mark_failure` with
        // `Attempt::Read`, where a busy answer does not count and pauses the
        // container (`busy_paused`).
        *self.pass_forcing.lock().expect("cache refresher poisoned") = forcing;
        self.pass_succeeded.store(false, Ordering::Relaxed);
        self.busy_families
            .lock()
            .expect("busy families poisoned")
            .clear();
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
        if !scope.only_contacts {
            self.enumerate_calendars(&mut targets).await;
            self.enumerate_task_lists(&mut targets).await;
        }
        self.enumerate_contact_lists(&mut targets, scope.read_only_directories)
            .await;
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
        // restart with a meaningful "last updated"). Not after a
        // contacts-only pass: the calendars and task lists were not read.
        if !scope.only_contacts {
            let completed = Utc::now();
            *self
                .last_refreshed
                .lock()
                .expect("cache refresher poisoned") = Some(completed);
            let repo = UserPrefsRepo::new(&self.db);
            let _ = repo.set(PREF_CACHE_LAST_REFRESHED_AT, &completed.to_rfc3339());
        }
        // The contacts sync is this pass: say so where its status lives.
        if let Some((core, observer)) = contact_sync {
            let accounts = std::mem::take(
                &mut *self
                    .contact_accounts
                    .lock()
                    .expect("cache refresher poisoned"),
            );
            let (succeeded, failed): (Vec<_>, Vec<_>) =
                accounts.into_iter().partition(|(_, ok)| *ok);
            core.finish_pass(
                succeeded.into_iter().map(|(id, _)| id).collect(),
                failed.into_iter().map(|(id, _)| id).collect(),
                &*observer,
            );
        }
        total
    }

    async fn enumerate_calendars(&self, out: &mut Vec<RefreshTarget>) {
        for (account, adapter) in self.registry.snapshot_calendar_adapters() {
            // Taken before the fetch: a write landing while it runs must not
            // be paused by its busy answer (decision 271).
            let generation = self
                .cache
                .refresh_generation(&account, SyncScope::Calendars, "");
            match adapter.list_calendars().await {
                Ok(cals) => {
                    self.pass_succeeded.store(true, Ordering::Relaxed);
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
                    self.failed(&account, SyncScope::Calendars, "", &err, generation);
                }
            }
        }
    }

    async fn enumerate_task_lists(&self, out: &mut Vec<RefreshTarget>) {
        for (account, adapter) in self.registry.snapshot_task_adapters() {
            // Taken before the fetch: a write landing while it runs must not
            // be paused by its busy answer (decision 271).
            let generation = self
                .cache
                .refresh_generation(&account, SyncScope::TaskLists, "");
            match adapter.list_task_lists().await {
                Ok(lists) => {
                    self.pass_succeeded.store(true, Ordering::Relaxed);
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
                    self.failed(&account, SyncScope::TaskLists, "", &err, generation);
                }
            }
        }
    }

    /// A contact account's lists came through (`true`), or one did not.
    fn contact_account(&self, account: &str, ok: bool) {
        let mut accounts = self
            .contact_accounts
            .lock()
            .expect("cache refresher poisoned");
        let entry = accounts.entry(account.to_string()).or_insert(true);
        *entry = *entry && ok;
    }

    async fn enumerate_contact_lists(
        &self,
        out: &mut Vec<RefreshTarget>,
        read_only_directories: bool,
    ) {
        for (account, adapter) in self.registry.snapshot_contact_adapters() {
            // Taken before the fetch: a write landing while it runs must not
            // be paused by its busy answer (decision 271).
            let generation = self
                .cache
                .refresh_generation(&account, SyncScope::ContactLists, "");
            match adapter.list_contact_lists().await {
                Ok(lists) => {
                    self.pass_succeeded.store(true, Ordering::Relaxed);
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
                    self.contact_account(&account, true);
                    for list in lists {
                        // A read-only directory is listed, but its contents
                        // are read only when this pass asks for them.
                        if list.read_only && !read_only_directories {
                            continue;
                        }
                        out.push(RefreshTarget::Contacts {
                            account: account.clone(),
                            adapter: adapter.clone(),
                            list_id: list.id,
                        });
                    }
                }
                Err(err) => {
                    self.contact_account(&account, false);
                    self.failed(&account, SyncScope::ContactLists, "", &err, generation);
                }
            }
        }
    }

    /// Refresh one container's items, deduped against the per-read SWR path via
    /// the shared coordinator. A claim miss means a per-read refresh is already
    /// handling it — skip without releasing (we never claimed).
    async fn refresh_one(&self, target: RefreshTarget, window: DateRange) {
        // A busy provider turned this family of the account away earlier in
        // the pass: asking its other containers now would only be turned away
        // too (decision 271).
        if self.family_busy(&target) {
            return;
        }
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
                        self.pass_succeeded.store(true, Ordering::Relaxed);
                        if changed {
                            self.emit_updated(SyncScope::Events, &account, &cal_id);
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Events, &cal_id, &err, generation);
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
                        self.pass_succeeded.store(true, Ordering::Relaxed);
                        if changed {
                            self.emit_updated(SyncScope::Tasks, &account, &list_id);
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Tasks, &list_id, &err, generation);
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
                        self.pass_succeeded.store(true, Ordering::Relaxed);
                        if changed {
                            self.emit_updated(SyncScope::Sections, &account, &list_id);
                        }
                    }
                    Err(err) => {
                        self.failed(&account, SyncScope::Sections, &list_id, &err, generation);
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
                        self.pass_succeeded.store(true, Ordering::Relaxed);
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
                        self.contact_account(&account, false);
                        self.failed(&account, SyncScope::Contacts, &list_id, &err, generation);
                    }
                }
                self.coord.release(&key);
            }
        }
    }

    /// Whether a busy provider turned this target's family of its account
    /// away earlier in the pass.
    fn family_busy(&self, target: &RefreshTarget) -> bool {
        let (account, scope) = match target {
            RefreshTarget::Events { account, .. } => (account, SyncScope::Events),
            RefreshTarget::Tasks { account, .. } => (account, SyncScope::Tasks),
            RefreshTarget::Sections { account, .. } => (account, SyncScope::Sections),
            RefreshTarget::Contacts { account, .. } => (account, SyncScope::Contacts),
        };
        self.busy_families
            .lock()
            .expect("busy families poisoned")
            .contains(&(account.clone(), super::listing_of(scope).as_str()))
    }

    /// Record a failed attempt of this pass, and log it when it is news:
    /// the container was not failing, or fails differently now. An account
    /// that keeps failing the same way says so once, not on every pass.
    ///
    /// `generation` is the one the attempt was claimed at; a listing, which
    /// is not claimed, gives the one it had when its fetch started.
    fn failed(
        &self,
        account: &str,
        scope: SyncScope,
        container: &str,
        err: &cal_core::Error,
        generation: u64,
    ) {
        let forced = self
            .pass_forcing
            .lock()
            .expect("cache refresher poisoned")
            .covers(account);
        let attempt = if forced {
            Attempt::Forced { generation }
        } else {
            Attempt::Background { generation }
        };
        if err.is_provider_busy() {
            self.busy_families
                .lock()
                .expect("busy families poisoned")
                .insert((account.to_string(), super::listing_of(scope).as_str()));
        }
        let news = self
            .cache
            .mark_failure(account, scope, container, err, attempt)
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
    /// is confirmed), and whether the pass, trying at least one account,
    /// read nothing at all. `None` when either could not be read; the
    /// surfaces then say a sentence that claims neither.
    fn pass_outcome(&self, scope: PassScope) -> Option<PassOutcome> {
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
        // What this pass asked: a contacts-only one asked no calendar or
        // task account, so their accounts must not make it "all failed".
        let (calendars, task_lists) = if scope.only_contacts {
            (Vec::new(), Vec::new())
        } else {
            (
                self.registry.snapshot_calendar_adapters(),
                self.registry.snapshot_task_adapters(),
            )
        };
        let attempted: std::collections::HashSet<String> = calendars
            .into_iter()
            .map(|(account, _)| account)
            .chain(task_lists.into_iter().map(|(a, _)| a))
            .chain(
                self.registry
                    .snapshot_contact_adapters()
                    .into_iter()
                    .map(|(a, _)| a),
            )
            .collect();
        // Not "every account has a failure": one failing calendar of six, or
        // a withheld calendar next to readable reminders, still updated
        // something, and "except: …" says what it did not.
        let all_failed = !attempted.is_empty() && !self.pass_succeeded.load(Ordering::Relaxed);
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

    /// A change of the refresh interval ends the wait at once, so it is
    /// measured again.
    #[tokio::test]
    async fn a_changed_interval_ends_the_wait_at_once() {
        let (refresher, _gated, _) = refresher();
        crate::user_prefs::UserPrefsRepo::new(&refresher.db)
            .set(sync_engine::PREF_SYNC_INTERVAL_MINUTES, "240")
            .unwrap();
        let waiting = {
            let refresher = Arc::clone(&refresher);
            tokio::spawn(async move { refresher.next_wake().await })
        };
        tokio::task::yield_now().await;
        refresher.interval_changed();
        let wake = tokio::time::timeout(StdDuration::from_secs(2), waiting)
            .await
            .expect("the wait ends at once, not after 240 minutes")
            .unwrap();
        assert_eq!(wake, Wake::Retime);
    }

    /// In the running loop, the re-timed wait runs no pass of its own: only
    /// the interval's own tick or a request does.
    #[tokio::test]
    async fn a_changed_interval_runs_no_pass_in_the_loop() {
        let (refresher, gated, statuses) = refresher();
        crate::user_prefs::UserPrefsRepo::new(&refresher.db)
            .set(sync_engine::PREF_SYNC_INTERVAL_MINUTES, "240")
            .unwrap();
        refresher.start_periodic(&tokio::runtime::Handle::current());
        // Cut the start delay short, and let the first pass through.
        refresher.trigger_background();
        gated.entered.notified().await;
        gated.release.notify_one();
        let ended = |statuses: &Statuses| {
            statuses
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|s| !s.refreshing)
                .count()
        };
        while ended(&statuses) == 0 {
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
        refresher.interval_changed();
        tokio::time::sleep(StdDuration::from_millis(200)).await;
        assert_eq!(gated.listings.load(Ordering::SeqCst), 1, "no second pass");
        assert_eq!(ended(&statuses), 1);
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

    /// A calendar account with two calendars, one of which always fails.
    struct HalfDown;

    #[async_trait]
    impl Adapter for HalfDown {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl CalendarFeature for HalfDown {
        async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
            Ok(["cal-ok", "cal-down"]
                .into_iter()
                .map(|id| Calendar {
                    color_label: None,
                    supports_scheduling: false,
                    supports_event_color: false,
                    always_notifies_attendees: false,
                    invitations_reply_only: false,
                    stores_occurrence_exceptions: false,
                    notifier_name: None,
                    id: id.into(),
                    name: id.into(),
                    color: None,
                    read_only: false,
                    default_sound: None,
                    invitation_import: Default::default(),
                    keeps_uid: false,
                })
                .collect())
        }
        async fn get_events(&self, calendar: &str, _: DateRange) -> cal_core::Result<Vec<Event>> {
            if calendar == "cal-down" {
                Err(cal_core::Error::Network("calendar unreachable".into()))
            } else {
                Ok(Vec::new())
            }
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

    /// An account whose ten calendars all answer that the provider is busy,
    /// and whose one task list answers; it counts what was asked.
    struct Throttled {
        events_asked: AtomicU32,
        tasks_asked: AtomicU32,
    }

    #[async_trait]
    impl Adapter for Throttled {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl CalendarFeature for Throttled {
        async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
            Ok((0..10)
                .map(|n| Calendar {
                    color_label: None,
                    supports_scheduling: false,
                    supports_event_color: false,
                    always_notifies_attendees: false,
                    invitations_reply_only: false,
                    stores_occurrence_exceptions: false,
                    notifier_name: None,
                    id: format!("cal-{n}"),
                    name: format!("Calendar {n}"),
                    color: None,
                    read_only: false,
                    default_sound: None,
                    invitation_import: Default::default(),
                    keeps_uid: false,
                })
                .collect())
        }
        async fn get_events(&self, _: &str, _: DateRange) -> cal_core::Result<Vec<Event>> {
            self.events_asked.fetch_add(1, Ordering::SeqCst);
            Err(cal_core::Error::provider_busy(
                cal_core::BusyDetail::TooManyRequests,
            ))
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

    #[async_trait]
    impl TasksFeature for Throttled {
        async fn list_task_lists(&self) -> cal_core::Result<Vec<cal_core::TaskList>> {
            Ok(vec![cal_core::TaskList {
                id: "list-1".into(),
                name: "Inbox".into(),
                color: None,
                read_only: false,
                parent_id: None,
                color_label: None,
                default_sound: None,
                embedded_in_calendar: None,
            }])
        }
        async fn get_tasks(&self, _: &str) -> cal_core::Result<Vec<cal_core::Task>> {
            self.tasks_asked.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn create_task(
            &self,
            _: &str,
            _: cal_core::NewTask,
        ) -> cal_core::Result<cal_core::Task> {
            unreachable!()
        }
        async fn update_task(&self, _: cal_core::Task) -> cal_core::Result<cal_core::Task> {
            unreachable!()
        }
        async fn delete_task(&self, _: &str) -> cal_core::Result<()> {
            unreachable!()
        }
    }

    /// A task list made while the listing is fetched moves the listing's
    /// generation on; the fetch's busy answer pauses at the generation it
    /// started at, so the new list's next read goes through (decision 271).
    struct ListsWrittenMidFetch {
        cache: Arc<CacheStore>,
    }

    #[async_trait]
    impl Adapter for ListsWrittenMidFetch {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl TasksFeature for ListsWrittenMidFetch {
        async fn list_task_lists(&self) -> cal_core::Result<Vec<cal_core::TaskList>> {
            self.cache
                .invalidate("acc-1", SyncScope::TaskLists, "")
                .unwrap();
            Err(cal_core::Error::provider_busy(
                cal_core::BusyDetail::TooManyRequests,
            ))
        }
        async fn get_tasks(&self, _: &str) -> cal_core::Result<Vec<cal_core::Task>> {
            unreachable!()
        }
        async fn create_task(
            &self,
            _: &str,
            _: cal_core::NewTask,
        ) -> cal_core::Result<cal_core::Task> {
            unreachable!()
        }
        async fn update_task(&self, _: cal_core::Task) -> cal_core::Result<cal_core::Task> {
            unreachable!()
        }
        async fn delete_task(&self, _: &str) -> cal_core::Result<()> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn a_listing_written_during_its_busy_fetch_is_not_paused() {
        let (refresher, _gated, _statuses) = refresher();
        // Without the gated calendars, which would hold the pass.
        refresher.registry.unregister("acc-1");
        refresher.registry.register_host_adapter(
            "acc-1",
            None,
            Some(Arc::new(ListsWrittenMidFetch {
                cache: Arc::clone(&refresher.cache),
            }) as Arc<dyn TasksFeature>),
        );
        refresher.warm_all(false).await;
        assert!(!refresher
            .cache
            .busy_paused("acc-1", SyncScope::TaskLists, ""));
    }

    /// Once a calendar of an account answers busy, the pass asks none of the
    /// account's other calendars (decision 271), records nothing for them,
    /// and still asks its task lists; the next pass, forced or not, asks
    /// again.
    #[tokio::test]
    async fn a_busy_answer_spares_the_rest_of_its_family_in_that_pass() {
        let (refresher, _gated, _statuses) = refresher();
        let throttled = Arc::new(Throttled {
            events_asked: AtomicU32::new(0),
            tasks_asked: AtomicU32::new(0),
        });
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(Arc::clone(&throttled) as Arc<dyn CalendarFeature>),
            Some(Arc::clone(&throttled) as Arc<dyn TasksFeature>),
        );
        let failing_rows = |refresher: &CacheRefresher| -> u32 {
            refresher
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM cache_sync_state
                     WHERE account_id = 'acc-1' AND scope = 'events' AND last_error IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        refresher.warm_all(false).await;
        let asked = throttled.events_asked.load(Ordering::SeqCst);
        assert!((1..10).contains(&asked), "asked {asked} of 10 calendars");
        assert_eq!(
            failing_rows(&refresher),
            asked,
            "the spared calendars record nothing"
        );
        assert_eq!(throttled.tasks_asked.load(Ordering::SeqCst), 1);
        // The next pass asks again, a forced one too.
        refresher.warm_all(true).await;
        let again = throttled.events_asked.load(Ordering::SeqCst) - asked;
        assert!(
            (1..10).contains(&again),
            "asked {again} of 10 calendars again"
        );
        assert_eq!(throttled.tasks_asked.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn one_failing_calendar_is_named_not_all_failed() {
        // Five of six calendars updated: "updated, except: Work", never
        // "nothing could be updated".
        let (refresher, _gated, statuses) = refresher();
        refresher.registry.register_host_adapter(
            "acc-1",
            Some(Arc::new(HalfDown) as Arc<dyn CalendarFeature>),
            None,
        );
        refresher.warm_all(true).await;
        let outcome = statuses.last_outcome();
        assert_eq!(
            outcome
                .failing
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Work"]
        );
        assert!(!outcome.all_failed, "one calendar was read");
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
                    rank: super::super::RefreshCause::Access.rank(),
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
        let blip = statuses.last_outcome();
        assert!(blip.failing.is_empty());
        // Nothing was read; the surfaces only say that the refresh ended,
        // since no failure is confirmed.
        assert!(blip.all_failed);
        refresher.warm_all(true).await;
        let outcome = statuses.last_outcome();
        assert_eq!(outcome.failing.len(), 1);
        assert_eq!(outcome.failing[0].cause, super::super::RefreshCause::Other);
        assert!(outcome.all_failed);
    }

    /// A change of one account forces only that account (Toni, 2026-10-23):
    /// an unchanged account that fails in the same pass counts as an
    /// automatic round and is not named after one.
    #[tokio::test]
    async fn an_account_change_forces_only_that_account() {
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
        for account in ["acc-1", "acc-2"] {
            refresher.registry.register_host_adapter(
                account,
                Some(Arc::new(Down) as Arc<dyn CalendarFeature>),
                None,
            );
        }
        refresher.warm_all(Forcing::account("acc-1")).await;
        let outcome = statuses.last_outcome();
        assert_eq!(
            outcome
                .failing
                .iter()
                .map(|f| f.account_id.as_str())
                .collect::<Vec<_>>(),
            vec!["acc-1"]
        );
    }

    /// Requests collapsed into one pass force every account either forced.
    #[test]
    fn forcing_merges_and_covers() {
        let one = Forcing::account("a").merge(Forcing::account("b"));
        assert!(one.covers("a") && one.covers("b") && !one.covers("c"));
        assert_eq!(
            Forcing::None.merge(Forcing::account("a")),
            Forcing::account("a")
        );
        assert_eq!(Forcing::account("a").merge(Forcing::All), Forcing::All);
        assert!(!Forcing::None.covers("a"));
        assert!(Forcing::All.covers("a"));
        assert_eq!(Forcing::from(true), Forcing::All);
        assert_eq!(Forcing::from(false), Forcing::None);
    }

    /// A queued request for one account and one for another run as one
    /// follow-up, forced for both.
    #[tokio::test]
    async fn queued_account_changes_collapse_into_one_follow_up() {
        let (refresher, gated, _statuses) = refresher();
        let running = tokio::spawn({
            let refresher = Arc::clone(&refresher);
            async move { refresher.warm_all(false).await }
        });
        gated.entered.notified().await;
        refresher.warm_all_queued(Forcing::account("acc-1")).await;
        refresher.warm_all_queued(Forcing::account("acc-2")).await;
        gated.release.notify_one();
        running.await.unwrap();
        let ran = refresher.pass_forcing.lock().unwrap().clone();
        assert!(ran.covers("acc-1") && ran.covers("acc-2"), "{ran:?}");
        assert!(!ran.covers("acc-3"), "{ran:?}");
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
        assert_eq!(
            *refresher.pass_forcing.lock().unwrap(),
            Forcing::All,
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

    /// An address book account: a writable book and a read-only directory.
    /// Records which lists a pass read; `fail` makes the book's read throw,
    /// `hold` holds the first read until the test lets it go.
    #[derive(Default)]
    struct Book {
        reads: Mutex<Vec<String>>,
        fail: std::sync::atomic::AtomicBool,
        hold: std::sync::atomic::AtomicBool,
        entered: Notify,
        release: Notify,
    }

    impl Book {
        fn reads(&self) -> Vec<String> {
            let mut reads = self.reads.lock().unwrap().clone();
            reads.sort();
            reads
        }
    }

    #[async_trait]
    impl Adapter for Book {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl ContactsFeature for Book {
        async fn list_contact_lists(&self) -> cal_core::Result<Vec<cal_core::ContactList>> {
            let list = |id: &str, read_only| cal_core::ContactList {
                id: id.into(),
                name: id.into(),
                color: None,
                color_label: None,
                read_only,
            };
            Ok(vec![list("book", false), list("directory", true)])
        }
        async fn get_contacts(&self, list_id: &str) -> cal_core::Result<Vec<cal_core::Contact>> {
            if self.hold.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.reads.lock().unwrap().push(list_id.to_string());
            if list_id == "book" && self.fail.load(Ordering::SeqCst) {
                return Err(cal_core::Error::Network("down".into()));
            }
            Ok(Vec::new())
        }
        async fn search_contacts(&self, _: &str) -> cal_core::Result<Vec<cal_core::Contact>> {
            Ok(Vec::new())
        }
        async fn create_contact(
            &self,
            _: &str,
            _: cal_core::NewContact,
        ) -> cal_core::Result<cal_core::Contact> {
            unreachable!()
        }
        async fn update_contact(
            &self,
            _: cal_core::Contact,
        ) -> cal_core::Result<cal_core::Contact> {
            unreachable!()
        }
        async fn delete_contact(&self, _: &str) -> cal_core::Result<()> {
            unreachable!()
        }
    }

    /// Every contacts-synced signal, in order.
    #[derive(Default)]
    struct Synced(Mutex<Vec<crate::contact_sync::ContactsSyncedPayload>>);

    impl ContactSyncObserver for Synced {
        fn contacts_synced(&self, payload: &crate::contact_sync::ContactsSyncedPayload) {
            self.0.lock().unwrap().push(payload.clone());
        }
    }

    struct BookSetUp {
        refresher: Arc<CacheRefresher>,
        calendars: Arc<Withheld>,
        book: Arc<Book>,
        contacts: Arc<ContactSyncCore>,
        synced: Arc<Synced>,
        statuses: Arc<Statuses>,
    }

    /// One account with a calendar (counted, never held) and an address book,
    /// and the contacts' status installed.
    fn book_refresher() -> BookSetUp {
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
        let calendars = Arc::new(Withheld {
            listings: AtomicU32::new(0),
            granted: std::sync::atomic::AtomicBool::new(true),
        });
        registry.register_host_adapter(
            "acc-1",
            Some(Arc::clone(&calendars) as Arc<dyn CalendarFeature>),
            None,
        );
        let book = Arc::new(Book::default());
        registry.register_host_contacts("acc-1", Arc::clone(&book) as Arc<dyn ContactsFeature>);
        let statuses = Arc::new(Statuses::default());
        let refresher = CacheRefresher::new(
            registry,
            Arc::new(CacheStore::new(db.clone())),
            Arc::new(RefreshCoordinator::new()),
            db.shared(),
            Arc::clone(&statuses) as Arc<dyn CacheObserver>,
        );
        let contacts = ContactSyncCore::new(db.shared());
        let synced = Arc::new(Synced::default());
        refresher.set_contact_sync(
            Arc::clone(&contacts),
            Arc::clone(&synced) as Arc<dyn ContactSyncObserver>,
        );
        BookSetUp {
            refresher,
            calendars,
            book,
            contacts,
            synced,
            statuses,
        }
    }

    #[test]
    fn pass_scopes_merge_and_cover() {
        let contacts = |read_only_directories| PassScope {
            only_contacts: true,
            read_only_directories,
        };
        let periodic = PassScope {
            only_contacts: false,
            read_only_directories: false,
        };
        assert_eq!(contacts(true).merge(periodic), PassScope::FULL);
        assert_eq!(contacts(false).merge(contacts(false)), contacts(false));
        assert!(PassScope::FULL.covers(periodic));
        assert!(PassScope::FULL.covers(contacts(true)));
        assert!(periodic.covers(contacts(false)));
        assert!(
            !periodic.covers(contacts(true)),
            "the directories were left out"
        );
        assert!(
            !contacts(true).covers(periodic),
            "the calendars were not read"
        );
    }

    /// A pass on the interval's schedule reads the read-only directory only
    /// when the switch is on; the app-start and a manual pass read it always.
    #[tokio::test]
    async fn a_periodic_pass_leaves_out_the_directories_unless_asked() {
        let set_up = book_refresher();
        set_up.refresher.warm_periodic().await;
        assert_eq!(set_up.book.reads(), vec!["book"]);

        set_up.book.reads.lock().unwrap().clear();
        set_up.refresher.warm_all(false).await;
        assert_eq!(set_up.book.reads(), vec!["book", "directory"]);

        set_up.book.reads.lock().unwrap().clear();
        crate::user_prefs::UserPrefsRepo::new(&set_up.refresher.db)
            .set(crate::contact_sync::PREF_INCLUDE_READ_ONLY_ON_SYNC, "true")
            .unwrap();
        set_up.refresher.warm_periodic().await;
        assert_eq!(set_up.book.reads(), vec!["book", "directory"]);
        assert_eq!(set_up.calendars.listings.load(Ordering::SeqCst), 3);
    }

    /// "Sync contacts now" reads the address books and nothing else, leaves
    /// "last updated" of the external data alone, and is the contacts sync:
    /// "last synced", and the done signal with the account.
    #[tokio::test]
    async fn sync_contacts_now_reads_only_contacts_and_reports_them() {
        let set_up = book_refresher();
        assert!(
            set_up.refresher.warm_contacts(false).await,
            "it ran at once"
        );
        assert_eq!(set_up.calendars.listings.load(Ordering::SeqCst), 0);
        assert_eq!(set_up.book.reads(), vec!["book"]);
        assert_eq!(set_up.refresher.status().last_refreshed_at, None);
        let status = set_up.contacts.status();
        assert!(!status.in_flight);
        let synced = set_up.synced.0.lock().unwrap();
        assert_eq!(synced.len(), 1);
        assert_eq!(
            Some(synced[0].last_synced_at.clone()),
            status.last_synced_at
        );
        assert_eq!(synced[0].succeeded_accounts, vec!["acc-1".to_string()]);
        assert!(synced[0].failed_accounts.is_empty());
        assert!(!set_up.statuses.last_outcome().all_failed);
    }

    /// A book that could not be read marks its account failed; a full pass
    /// says "synced" too.
    #[tokio::test]
    async fn an_unreadable_book_reports_its_account_failed() {
        let set_up = book_refresher();
        set_up.book.fail.store(true, Ordering::SeqCst);
        set_up.refresher.warm_all(false).await;
        let synced = set_up.synced.0.lock().unwrap();
        assert_eq!(synced.len(), 1);
        assert!(synced[0].succeeded_accounts.is_empty());
        assert_eq!(synced[0].failed_accounts, vec!["acc-1".to_string()]);
    }

    /// A contacts-only pass with calendar accounts and no address book asked
    /// nothing of those accounts: it is not "nothing could be updated".
    #[tokio::test]
    async fn a_contacts_pass_without_books_is_not_all_failed() {
        let (refresher, gated, statuses) = refresher();
        refresher.warm_contacts(false).await;
        assert_eq!(gated.listings.load(Ordering::SeqCst), 0);
        assert!(!statuses.last_outcome().all_failed);
    }

    /// A tick that lands during a contacts-only pass is not dropped: that pass
    /// reads no calendars, so the tick's pass follows it.
    #[tokio::test]
    async fn a_tick_during_a_contacts_pass_follows_it() {
        let set_up = book_refresher();
        set_up.book.hold.store(true, Ordering::SeqCst);
        let running = tokio::spawn({
            let refresher = Arc::clone(&set_up.refresher);
            async move { refresher.warm_contacts(false).await }
        });
        set_up.book.entered.notified().await;
        set_up.refresher.warm_periodic().await;
        set_up.book.release.notify_one();
        assert!(running.await.unwrap());
        assert_eq!(
            set_up.calendars.listings.load(Ordering::SeqCst),
            1,
            "the tick's pass followed"
        );
        assert!(set_up.refresher.status().last_refreshed_at.is_some());
        assert_eq!(set_up.synced.0.lock().unwrap().len(), 2);
    }

    /// "Sync contacts now" during a running pass waits for it and runs after.
    #[tokio::test]
    async fn sync_contacts_now_during_a_pass_follows_it() {
        let set_up = book_refresher();
        set_up.book.hold.store(true, Ordering::SeqCst);
        let running = tokio::spawn({
            let refresher = Arc::clone(&set_up.refresher);
            async move { refresher.warm_periodic().await }
        });
        set_up.book.entered.notified().await;
        assert!(
            set_up.contacts.status().in_flight,
            "the pass reads the books"
        );
        assert!(!set_up.refresher.warm_contacts(false).await, "queued");
        set_up.book.release.notify_one();
        running.await.unwrap();
        assert_eq!(set_up.book.reads(), vec!["book", "book"]);
        assert_eq!(set_up.calendars.listings.load(Ordering::SeqCst), 1);
        assert_eq!(set_up.synced.0.lock().unwrap().len(), 2);
        assert!(!set_up.contacts.status().in_flight);
    }
}
