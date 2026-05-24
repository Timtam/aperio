//! Sync scheduler — Phase Se's automatic-trigger layer for the
//! orchestrator built in Phase Sd.
//!
//! Phase Sd shipped a manual `sync_now` command. Phase Se covers the
//! other four triggers DESIGN.md §19.8 names:
//!
//! | Trigger     | How this module fires it                                 |
//! |-------------|----------------------------------------------------------|
//! | App start   | Single delayed kick (`APP_START_DELAY`) after boot.      |
//! | Periodic    | `tokio::time::sleep(interval)` loop, default 5 min,      |
//! |             | configurable via `user_prefs.sync.intervalMinutes`.       |
//! | On mutation | `EventLogWriter::append` pings a shared `Notify`; the    |
//! |             | scheduler debounces a `DEBOUNCE_WINDOW` and then pushes. |
//! | App exit    | `lib.rs` blocks on a final push via the orchestrator's    |
//! |             | `push_pending` path on `RunEvent::ExitRequested`.        |
//! | Manual      | Already covered by the `sync_now` Tauri command — the    |
//! |             | scheduler doesn't intercept manual triggers, just resets |
//! |             | the interval clock so the next periodic tick measures    |
//! |             | from the manual round.                                    |
//!
//! ## Why one extra layer on top of the orchestrator?
//!
//! The orchestrator is `async`-friendly but doesn't own a timer.
//! Putting the periodic / debounce logic into a dedicated scheduler
//! mirrors the [`ContactSyncScheduler`] pattern and keeps each
//! component testable in isolation: the orchestrator can be exercised
//! against fake adapters without spinning a real-time clock, and the
//! scheduler's policy decisions (when to sync) live in one place.
//!
//! ## In-flight dedupe
//!
//! The orchestrator already has its own in-flight guard
//! (`SyncOrchestrator::sync_now` returns an `"already in progress"`
//! error when called concurrently). The scheduler doesn't double up
//! on that — it just lets the orchestrator reject the duplicate.
//! Concurrent kicks coalesce in the debounce window instead.
//!
//! ## What the scheduler does NOT do
//!
//! - **Network detection.** "Bei jeder Änderung … sofern Verbindung
//!   vorhanden" (§19.8) is honoured implicitly: the adapter's
//!   `push_log` returns an `Io` error when the path is unreachable,
//!   the scheduler logs + moves on, and the next kick / periodic tick
//!   retries. A proper online/offline indicator lives in Phase Si.
//! - **Backoff.** Failed rounds don't slow the next attempt. Phase Sj
//!   (WebDAV) needs proper exponential backoff against rate limits;
//!   the LocalFsSyncAdapter doesn't.
//! - **Snapshot generation.** Phase Sg.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sync_core::SyncResult;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::event_log::{SyncOrchestrator, SyncRoundReport, SyncStatus};
use crate::sync_log::{SyncLogCounters, SyncLogRepo, SyncTrigger};
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key for the configurable polling interval, in minutes.
/// Defaults to [`DEFAULT_SYNC_INTERVAL_MINUTES`] — matches the value
/// DESIGN.md §19.8 names as the standard cadence.
pub const PREF_SYNC_INTERVAL_MINUTES: &str = "sync.intervalMinutes";

/// Default interval if `PREF_SYNC_INTERVAL_MINUTES` is unset or
/// unparseable. 5 minutes per DESIGN.md §19.8.
pub const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 5;

/// How long after app start the first sync round kicks off. Lets the
/// Tauri main thread finish wiring everything up + lets the UI paint
/// before we start consuming bandwidth.
const APP_START_DELAY: Duration = Duration::from_secs(5);

/// Coalescing window for mutation-triggered kicks. A burst of
/// mutations (paste-many-events, bulk delete, drag-multiple) pings
/// the kick `Notify` rapidly — the scheduler waits this long after
/// the latest kick before actually running the round. Keeps a one-
/// shot pile of edits inside a single push instead of N small ones.
const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);

/// Upper bound on the exponential backoff between failed rounds.
/// Failed rounds double the wait (2x, 4x, 8x …) up to this cap so
/// a remote that's been down for hours doesn't keep slamming the
/// network every few minutes. We pick 30 minutes as the ceiling —
/// large enough to noticeably back off, short enough that a
/// recovered network gets picked up within "one coffee break".
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// Threshold (consecutive failures) before we flip the SyncStatus
/// `sustained_failure` flag. Two-in-a-row is normal transient
/// noise; three signals "something's actually wrong" — the status
/// indicator can shift tone and a Settings reader gets a hint
/// without the user having to read the log.
const SUSTAINED_FAILURE_THRESHOLD: u32 = 3;

/// Frontend payload emitted on the `sync-status` channel. Sent
/// before + after every sync round so the status indicator can flip
/// between "↑ Wird hochgeladen…" and "✓ Synchronisiert" without
/// polling.
///
/// The payload mirrors [`SyncStatus`] plus an optional
/// [`SyncRoundReport`] for the post-round emit. Pre-round emits set
/// `report = None`.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatusPayload {
    #[serde(flatten)]
    pub status: SyncStatus,
    /// Counters from the just-completed sync round, when this emit
    /// follows a `sync_now`. `None` on the "round started" emit.
    pub report: Option<SyncRoundReport>,
    /// Set when the round failed at the orchestrator level (no
    /// adapter, network error). Per-file failures don't land here —
    /// they're counted in `report.apply_failures` / `push_failures`.
    pub error: Option<String>,
}

/// The scheduler itself. One per process, stored as managed state so
/// the `sync_now` / `set_sync_interval` commands can interact with it.
pub struct SyncScheduler {
    orchestrator: Arc<SyncOrchestrator>,
    db: SharedConn,
    /// Kick channel shared with the [`EventLogWriter`]. The writer's
    /// `append()` calls `notify_one`; the scheduler's loop wakes,
    /// debounces, and triggers a round.
    kick: Arc<Notify>,
    /// Track whether the periodic loop has finished its initial
    /// app-start kick. Used by tests so they don't have to wait the
    /// `APP_START_DELAY` to assert behaviour.
    started: Arc<Mutex<bool>>,
    /// Count of consecutive failed rounds (since the last success).
    /// Drives exponential backoff (`interval << min(n, cap)`) and
    /// the `sustained_failure` flag on `SyncStatus`. Reset to 0 by
    /// every successful round — including the manual `sync_now`
    /// command path so the user clicking "Sync now" after a hiccup
    /// gets immediate back-to-normal cadence.
    consecutive_failures: Arc<Mutex<u32>>,
}

impl SyncScheduler {
    /// Construct + spawn the background worker. The returned `Arc`
    /// is registered into Tauri State by `lib.rs` so commands can
    /// borrow it.
    ///
    /// Splitting the scheduler from the writer keeps both
    /// independently testable: the writer doesn't need a tokio
    /// runtime, and the scheduler doesn't need a real disk path.
    pub fn spawn<R: Runtime>(
        orchestrator: Arc<SyncOrchestrator>,
        db: SharedConn,
        kick: Arc<Notify>,
        app: AppHandle<R>,
    ) -> Arc<Self> {
        let scheduler = Arc::new(Self {
            orchestrator,
            db,
            kick,
            started: Arc::new(Mutex::new(false)),
            consecutive_failures: Arc::new(Mutex::new(0)),
        });

        let worker = scheduler.clone();
        tauri::async_runtime::spawn(async move {
            // App-start kick — wait for the UI to paint before pulling.
            // Skip if no adapter is configured: there's nothing to do
            // and the orchestrator would return "not configured"
            // straight away. The next periodic tick re-checks; once
            // the user configures an adapter, the configure command
            // explicitly kicks us.
            tokio::time::sleep(APP_START_DELAY).await;
            {
                let mut g = worker.started.lock().expect("started mutex poison");
                *g = true;
            }
            if worker.orchestrator.status().configured {
                info!("running app-start sync round");
                worker.run_round(&app, SyncTrigger::AppStart).await;
            } else {
                debug!("app-start sync skipped — no adapter configured");
            }

            // Main loop. tokio::select! gives us three wake-up paths:
            //   - the periodic timer fires → full round
                //   - the kick `Notify` fires → debounce, then round
            //   - both fire close together → handle the first one;
            //     the second remains pending and wakes the next
            //     iteration immediately (Notify is a one-shot
            //     latch, so a single kick is enough)
            loop {
                let interval = worker.interval_duration();
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {
                        if !worker.orchestrator.status().configured {
                            debug!("periodic sync skipped — no adapter configured");
                            continue;
                        }
                        debug!(?interval, "periodic sync tick");
                        worker.run_round(&app, SyncTrigger::Periodic).await;
                    }
                    _ = worker.kick.notified() => {
                        // Debounce. Sleep a short window, draining
                        // any further kicks that come in via
                        // `notified().now_or_never()` — we only
                        // care that *something* changed, not how
                        // many times.
                        tokio::time::sleep(DEBOUNCE_WINDOW).await;
                        if !worker.orchestrator.status().configured {
                            debug!("kick-triggered sync skipped — no adapter configured");
                            continue;
                        }
                        debug!("mutation-triggered sync round");
                        worker.run_round(&app, SyncTrigger::Kick).await;
                    }
                }
            }
        });
        scheduler
    }

    /// Run one sync round through the orchestrator and emit a
    /// `sync-status` event before + after. Errors don't bubble — the
    /// status emit carries the message instead.
    ///
    /// Maintains the `consecutive_failures` counter that drives the
    /// exponential backoff + `sustained_failure` status latch. A
    /// successful round resets it; a failed one bumps it. The
    /// orchestrator's "AlreadyRunning" rejection (returned when the
    /// in-flight guard is held) is NOT counted as a failure — it
    /// just means another round is already covering us.
    ///
    /// Also appends one row to the §19.9 Sync-Protokoll table —
    /// the Settings → Synchronisation → Protokoll list reads from
    /// there. `trigger` tags WHY this round ran so the user can
    /// filter manual / periodic / startup attempts apart in the
    /// rare bug-report scenario where it matters.
    async fn run_round<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        trigger: SyncTrigger,
    ) {
        self.emit_status(app, None, None);
        let started = Instant::now();
        let result = self.orchestrator.sync_now().await;
        let duration_ms = u64::try_from(started.elapsed().as_millis())
            .unwrap_or(u64::MAX);
        match &result {
            Ok(report) => {
                info!(
                    pushed = report.pushed_logs,
                    fetched = report.fetched_logs,
                    applied = report.applied,
                    conflicts = report.conflicts,
                    "scheduled sync round completed",
                );
                self.reset_failures();
                self.emit_status(app, Some(report.clone()), None);
                // New conflicts landed during this round — kick
                // the frontend's conflict-count fetch so the
                // status badge updates without polling. (The
                // applier writes to `sync_conflicts` but doesn't
                // emit the event itself; doing it here keeps the
                // applier handle-free.)
                if report.conflicts > 0 {
                    if let Err(err) = app.emit("sync-conflicts-changed", ()) {
                        warn!(?err, "failed to emit sync-conflicts-changed");
                    }
                }
            }
            Err(err) => {
                warn!(?err, "scheduled sync round failed");
                // `AlreadyRunning` is a courteous self-reject, not a
                // real failure — the round in flight will set the
                // success / failure tone for us.
                if !err.to_string().contains("AlreadyRunning") {
                    self.bump_failure();
                }
                self.emit_status(app, None, Some(err.to_string()));
            }
        }
        self.write_sync_log(app, trigger, &result, duration_ms);
    }

    /// Append one row to the §19.9 sync_log table + emit a
    /// `sync-log-changed` event so the Protokoll component in
    /// Settings refreshes without polling.
    ///
    /// Exposed via [`Self::record_manual_outcome`] so the manual
    /// `sync_now` Tauri command + the app-exit `push_now` path
    /// (which run through the orchestrator directly, not via the
    /// scheduler loop) can also contribute to the protocol.
    fn write_sync_log<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        trigger: SyncTrigger,
        result: &SyncResult<SyncRoundReport>,
        duration_ms: u64,
    ) {
        let (success, counters, error) = match result {
            Ok(report) => (
                true,
                SyncLogCounters {
                    pushed_logs: Some(u32::try_from(report.pushed_logs).unwrap_or(u32::MAX)),
                    fetched_logs: Some(u32::try_from(report.fetched_logs).unwrap_or(u32::MAX)),
                    applied: Some(u32::try_from(report.applied).unwrap_or(u32::MAX)),
                    conflicts: Some(
                        u32::try_from(report.conflicts).unwrap_or(u32::MAX),
                    ),
                },
                None,
            ),
            Err(err) => (false, SyncLogCounters::default(), Some(err.to_string())),
        };
        let repo = SyncLogRepo::new(&self.db);
        if let Err(err) = repo.record(
            trigger,
            success,
            &counters,
            Some(duration_ms),
            error.as_deref(),
        ) {
            // Persisting the protocol entry is best-effort; we
            // already emitted `sync-status` so the user sees the
            // outcome. Silently swallowing a sqlite hiccup beats
            // crashing the scheduler loop.
            warn!(?err, "couldn't persist sync_log entry");
            return;
        }
        // Frontend listens for `sync-log-changed` to refresh the
        // Protokoll list. No payload needed — the listener
        // re-fetches via `list_sync_log_entries`.
        if let Err(err) = app.emit("sync-log-changed", ()) {
            warn!(?err, "failed to emit sync-log-changed event");
        }
    }

    /// Public counterpart of [`Self::write_sync_log`] for the
    /// manual `sync_now` Tauri command + the app-exit push path.
    /// The scheduler loop uses `write_sync_log` directly; callers
    /// that don't go through the loop call this so their outcome
    /// still shows up in the Settings Protokoll.
    pub fn record_manual_outcome<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        trigger: SyncTrigger,
        result: &SyncResult<SyncRoundReport>,
        duration_ms: u64,
    ) {
        self.write_sync_log(app, trigger, result, duration_ms);
    }

    /// Bump the consecutive-failures counter; caps at u32::MAX (we
    /// only ever use the value to compute `1 << min(n, cap)` so the
    /// raw size doesn't matter beyond the backoff cap).
    fn bump_failure(&self) {
        let mut guard = self
            .consecutive_failures
            .lock()
            .expect("consecutive_failures mutex poison");
        *guard = guard.saturating_add(1);
    }

    /// Reset the consecutive-failures counter to zero. Called after
    /// every successful round; also exposed via [`Self::note_success`]
    /// so the manual `sync_now` Tauri command can clear the latch
    /// without going through the scheduler loop.
    fn reset_failures(&self) {
        let mut guard = self
            .consecutive_failures
            .lock()
            .expect("consecutive_failures mutex poison");
        *guard = 0;
    }

    /// Public accessor for the failure counter. Used by
    /// [`SyncOrchestrator::status`] to surface `sustained_failure`
    /// in the snapshot without coupling the orchestrator to the
    /// scheduler's internals.
    pub fn consecutive_failures(&self) -> u32 {
        *self
            .consecutive_failures
            .lock()
            .expect("consecutive_failures mutex poison")
    }

    /// Hook so the manual `sync_now` Tauri command (which runs
    /// through the orchestrator directly, not via the scheduler
    /// loop) can clear the latch after a successful round. Failure
    /// is still tracked only by the scheduler so a user-driven
    /// retry that fails doesn't keep them stuck in the warning
    /// state — they get a fresh chance on the next periodic tick.
    pub fn note_success(&self) {
        self.reset_failures();
    }

    fn emit_status<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        report: Option<SyncRoundReport>,
        error: Option<String>,
    ) {
        let payload = SyncStatusPayload {
            status: self.current_status(),
            report,
            error,
        };
        if let Err(err) = app.emit("sync-status", &payload) {
            warn!(?err, "failed to emit sync-status event");
        }
    }

    /// Augmented [`SyncStatus`] snapshot: orchestrator's view +
    /// the scheduler's `sustained_failure` decoration. Used by
    /// both the emit path and the `get_sync_status` Tauri
    /// command so the frontend gets the same picture whether it
    /// polls or listens.
    pub fn current_status(&self) -> SyncStatus {
        let mut status = self.orchestrator.status();
        status.sustained_failure =
            self.consecutive_failures() >= SUSTAINED_FAILURE_THRESHOLD;
        status
    }

    /// Read the configured interval from `user_prefs`. Defaults to
    /// [`DEFAULT_SYNC_INTERVAL_MINUTES`] when missing or unparseable.
    /// Values below 1 minute clamp to 1 so a typo (`0`) can't pin
    /// the worker into a hot loop.
    pub fn read_interval_minutes(&self) -> u32 {
        read_interval_minutes(&self.db)
    }

    /// Convert the configured interval into a `Duration`. When
    /// consecutive failures are non-zero, the base interval is
    /// shifted left by `min(failures, 5)` bits — yielding 1x, 2x,
    /// 4x, 8x, 16x, 32x — then capped at [`MAX_BACKOFF`]. The cap
    /// matters more than the exponent: it's what stops a remote
    /// that's been unreachable for a day from sleeping forever.
    ///
    /// We compute the multiplier as `1u32.checked_shl(n)`; any `n`
    /// beyond 31 would otherwise overflow and silently wrap to
    /// zero. The 5-bit ceiling keeps the math comfortably inside
    /// safe range AND already saturates at the MAX_BACKOFF cap for
    /// any base interval we'd realistically configure.
    fn interval_duration(&self) -> Duration {
        let base =
            Duration::from_secs(u64::from(self.read_interval_minutes()) * 60);
        backoff_duration(base, self.consecutive_failures())
    }

    /// Wake the background loop. Used by the `configure_sync_adapter`
    /// command so a fresh adapter triggers an immediate first round
    /// without the user having to click `sync_now` manually.
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    /// Update the configured interval. Wakes the loop so the new
    /// interval applies on the next sleep, not at the end of the
    /// current one.
    ///
    /// Returns the clamped value actually persisted, so the frontend
    /// can echo it back into the settings UI.
    pub fn set_interval_minutes(&self, minutes: u32) -> Result<u32, String> {
        let clamped = minutes.max(1);
        UserPrefsRepo::new(&self.db)
            .set(PREF_SYNC_INTERVAL_MINUTES, &clamped.to_string())
            .map_err(|err| err.to_string())?;
        // Reset the interval clock by faking a kick. The loop will
        // wake, hit the kick branch, debounce, and (since nothing
        // mutated) push an empty pending dir — almost free. The
        // alternative would be a second notify just for the
        // interval, which complicates the loop for little gain.
        self.kick.notify_one();
        Ok(clamped)
    }
}

/// Pure backoff calculation. Pulled out of
/// [`SyncScheduler::interval_duration`] so the tests can exercise
/// the math without spinning up a full scheduler.
///
/// `base` is the configured interval; `failures` is the count of
/// consecutive failed rounds since the last success. The result
/// is `base << min(failures, 5)`, capped at [`MAX_BACKOFF`]. The
/// 5-bit shift cap keeps the math inside `u32` range; the
/// MAX_BACKOFF cap is what actually bounds the sleep duration for
/// long-running outages.
fn backoff_duration(base: Duration, failures: u32) -> Duration {
    if failures == 0 {
        return base;
    }
    let shift = failures.min(5);
    let multiplier = 1u32 << shift;
    let scaled = base.saturating_mul(multiplier);
    scaled.min(MAX_BACKOFF)
}

/// Free function reading the configured interval. Exposed separately
/// from [`SyncScheduler::read_interval_minutes`] so callers that
/// don't hold a scheduler handle (e.g. the `get_sync_status` command
/// when no scheduler State exists in tests) can resolve the same
/// value.
pub fn read_interval_minutes(db: &SharedConn) -> u32 {
    UserPrefsRepo::new(db)
        .get(PREF_SYNC_INTERVAL_MINUTES)
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|m| m.max(1))
        .unwrap_or(DEFAULT_SYNC_INTERVAL_MINUTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn read_interval_minutes_defaults_to_five() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        assert_eq!(
            read_interval_minutes(&shared),
            DEFAULT_SYNC_INTERVAL_MINUTES,
        );
    }

    #[test]
    fn read_interval_minutes_honours_user_pref() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_SYNC_INTERVAL_MINUTES, "30")
            .unwrap();
        assert_eq!(read_interval_minutes(&shared), 30);
    }

    #[test]
    fn read_interval_minutes_clamps_zero_to_one() {
        // Defensive: a typo of `0` mustn't pin the worker into a
        // hot loop. We promote it to 1 so the round runs once a
        // minute at worst.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_SYNC_INTERVAL_MINUTES, "0")
            .unwrap();
        assert_eq!(read_interval_minutes(&shared), 1);
    }

    #[test]
    fn read_interval_minutes_falls_back_on_garbage() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_SYNC_INTERVAL_MINUTES, "not-a-number")
            .unwrap();
        assert_eq!(
            read_interval_minutes(&shared),
            DEFAULT_SYNC_INTERVAL_MINUTES,
        );
    }

    // -----------------------------------------------------------------
    // backoff_duration
    // -----------------------------------------------------------------

    #[test]
    fn backoff_zero_failures_returns_base_unchanged() {
        let base = Duration::from_secs(300); // 5 min
        assert_eq!(backoff_duration(base, 0), base);
    }

    #[test]
    fn backoff_doubles_per_failure() {
        let base = Duration::from_secs(60); // 1 min
        assert_eq!(backoff_duration(base, 1), Duration::from_secs(120));
        assert_eq!(backoff_duration(base, 2), Duration::from_secs(240));
        assert_eq!(backoff_duration(base, 3), Duration::from_secs(480));
    }

    #[test]
    fn backoff_caps_at_max_backoff() {
        // 5-min base, 5 failures → 5 * 32 = 160 min, way above
        // the 30-min cap.
        let base = Duration::from_secs(5 * 60);
        assert_eq!(backoff_duration(base, 5), MAX_BACKOFF);
        // Big shift cap: 100 failures still respects the cap.
        assert_eq!(backoff_duration(base, 100), MAX_BACKOFF);
    }

    #[test]
    fn backoff_doesnt_undershoot_with_tiny_base() {
        // 1-min base × 2 failures → 4 min, well below cap.
        let base = Duration::from_secs(60);
        assert_eq!(backoff_duration(base, 2), Duration::from_secs(240));
    }

    #[test]
    fn backoff_caps_when_shift_overflow_would_lose_precision() {
        // Guard the saturating_mul + cap interaction: with a base
        // large enough that even a single doubling exceeds the
        // cap, we still get the cap back, not zero.
        let big_base = MAX_BACKOFF; // 30 min
        assert_eq!(backoff_duration(big_base, 1), MAX_BACKOFF);
    }
}
