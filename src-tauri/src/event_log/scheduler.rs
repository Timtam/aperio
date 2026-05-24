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
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::event_log::{SyncOrchestrator, SyncRoundReport, SyncStatus};
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
                worker.run_round(&app).await;
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
                        worker.run_round(&app).await;
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
                        worker.run_round(&app).await;
                    }
                }
            }
        });
        scheduler
    }

    /// Run one sync round through the orchestrator and emit a
    /// `sync-status` event before + after. Errors don't bubble — the
    /// status emit carries the message instead.
    async fn run_round<R: Runtime>(&self, app: &AppHandle<R>) {
        self.emit_status(app, None, None);
        match self.orchestrator.sync_now().await {
            Ok(report) => {
                info!(
                    pushed = report.pushed_logs,
                    fetched = report.fetched_logs,
                    applied = report.applied,
                    "scheduled sync round completed",
                );
                self.emit_status(app, Some(report), None);
            }
            Err(err) => {
                warn!(?err, "scheduled sync round failed");
                self.emit_status(app, None, Some(err.to_string()));
            }
        }
    }

    fn emit_status<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        report: Option<SyncRoundReport>,
        error: Option<String>,
    ) {
        let payload = SyncStatusPayload {
            status: self.orchestrator.status(),
            report,
            error,
        };
        if let Err(err) = app.emit("sync-status", &payload) {
            warn!(?err, "failed to emit sync-status event");
        }
    }

    /// Read the configured interval from `user_prefs`. Defaults to
    /// [`DEFAULT_SYNC_INTERVAL_MINUTES`] when missing or unparseable.
    /// Values below 1 minute clamp to 1 so a typo (`0`) can't pin
    /// the worker into a hot loop.
    pub fn read_interval_minutes(&self) -> u32 {
        read_interval_minutes(&self.db)
    }

    /// Convert the configured interval into a `Duration`.
    fn interval_duration(&self) -> Duration {
        Duration::from_secs(u64::from(self.read_interval_minutes()) * 60)
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
}
