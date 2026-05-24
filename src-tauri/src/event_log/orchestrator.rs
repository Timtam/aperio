//! Sync orchestrator — Phase Sd's "one round of sync" coordinator.
//!
//! Pulls together the three Sa/Sb/Sc components into a single
//! `sync_now()` operation:
//!
//! ```text
//!  sync_now()
//!      │
//!      ▼  1. Push every file from
//!      │     <data_dir>/sync/log/pending/ to the remote via
//!      │     SyncAdapter::push_log.
//!      │
//!      ▼  2. Fetch every log from the remote whose timestamp
//!      │     is newer than our cursor (user_prefs.sync.cursor).
//!      │     Filter out files our own device originally wrote
//!      │     (they round-trip the loopback unchanged).
//!      │
//!      ▼  3. Hand each fetched LogFile to the EventLogApplier.
//!      │     Idempotency in `sync_applied_events` covers the
//!      │     "we already processed this file" case.
//!      │
//!      ▼  4. Advance the cursor to the latest log timestamp
//!      │     just fetched. Persist to user_prefs so the next
//!      │     round picks up where we left off.
//!      │
//!      ▼  5. Return a SyncRoundReport summarising what happened.
//! ```
//!
//! ## What's deliberately NOT in this orchestrator yet
//!
//! - **Periodic clock.** Phase Sd is manual-trigger-only. The
//!   scheduler that fires `sync_now()` every N minutes lives in
//!   Phase Se alongside app-start + on-mutation auto-push.
//! - **Snapshot generation + compaction.** Phase Sg. We pass
//!   `fetch_snapshot` through but never produce one.
//! - **Conflict resolution UI.** The applier's last-write-wins
//!   already produces a coherent state; surfacing field-level
//!   collisions for the user to choose between is Phase Sh.
//! - **Meta.json device registration.** Phase Sf — the
//!   onboarding flow does the upsert of our own DeviceRecord.
//! - **E2E encryption layer.** Phase Sk wraps the adapter calls
//!   with AES-256-GCM before bytes hit the SyncAdapter trait.
//! - **Multiple adapter kinds at once.** v1 picks one
//!   configured adapter; switching adapters requires a manual
//!   "clear cursor, re-onboard" gesture.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sync_core::{DeviceCursor, DeviceId, LogFile, SyncAdapter, SyncResult};
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::event_log::{ApplyReport, Compactor, EventLogApplier, OnboardingService};
use crate::user_prefs::UserPrefsRepo;

/// Bring the scheduler's interval pref into status reads. The
/// orchestrator owns the `SyncStatus` shape; reading the interval
/// from the same source-of-truth keeps the frontend from having to
/// query two endpoints.
use crate::event_log::scheduler::read_interval_minutes;

/// `user_prefs` key holding the RFC 3339 timestamp of the
/// newest log file we've already fetched from the remote. The
/// orchestrator reads this on entry to filter remote logs and
/// writes it back on success.
pub const SYNC_CURSOR_PREF_KEY: &str = "sync.cursor.lastSeenLog";

/// Result of one `sync_now()` invocation. Surfaced to the
/// frontend via the `sync_now` Tauri command so the user
/// dialogue can show "12 events applied" or "no new changes".
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct SyncRoundReport {
    /// Number of pending log files we successfully pushed to the
    /// remote.
    pub pushed_logs: usize,
    /// Number of log files we pulled from the remote (after
    /// cursor + own-device filtering).
    pub fetched_logs: usize,
    /// Aggregate apply counts across every fetched log.
    pub applied: usize,
    pub skipped_own: usize,
    pub skipped_already_applied: usize,
    pub skipped_unsupported: usize,
    /// Per-envelope failures inside the applier. Non-fatal —
    /// the round itself still counts as a success.
    pub apply_failures: usize,
    /// Push failures we logged but kept going on. Same
    /// philosophy as the applier: one bad file shouldn't sink
    /// the entire sync round.
    pub push_failures: usize,
    /// Field-level conflicts the applier recorded during this
    /// round (DESIGN.md §19.3). The scheduler reads this to
    /// decide whether to fire a §19.9 system notification +
    /// emit `sync-conflicts-changed` so the status bar refreshes
    /// its badge count.
    pub conflicts: usize,
}

impl SyncRoundReport {
    fn merge_apply(&mut self, report: ApplyReport) {
        self.applied += report.applied;
        self.skipped_own += report.skipped_own;
        self.skipped_already_applied += report.skipped_already_applied;
        self.skipped_unsupported += report.skipped_unsupported;
        self.apply_failures += report.failed;
        self.conflicts += report.conflicts;
    }
}

/// Read-only snapshot of the orchestrator's state, returned by
/// `get_sync_status`. The scheduler in Phase Se will extend this
/// with the next scheduled tick + interval; the Sd version
/// carries just enough for a "last synced at …" indicator.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub configured: bool,
    pub in_flight: bool,
    pub last_synced_at: Option<String>,
    /// Currently-configured periodic interval, in minutes. Surfaced
    /// alongside the other status bits so the Settings → Sync panel
    /// can render the slider value without a second round-trip.
    pub interval_minutes: u32,
    /// Whether the current dataset is end-to-end encrypted (Phase
    /// Sk). Read straight from `user_prefs.sync.adapter.e2eEnabled`
    /// — kept in sync by the onboarding + configure paths so the
    /// status indicator can flip a "🔒 verschlüsselt" badge on
    /// without a network call.
    pub e2e_enabled: bool,
    /// Phase Sl: latched `true` when the last sync round failed
    /// with `SyncError::SchemaTooOld`. The frontend pops the
    /// §19.13 "update required" modal off this flag; clears on
    /// the next successful round.
    pub schema_too_old: bool,
    /// When `schema_too_old`, the dataset's `min_app_version`
    /// requirement. Shown verbatim in the update prompt so the
    /// user knows what version they need. `None` when sync is
    /// fine.
    pub min_app_version_required: Option<String>,
    /// `true` when the scheduler has seen three or more
    /// consecutive failed rounds. Drives a warning tone on the
    /// status indicator + a banner in the Settings panel so the
    /// user doesn't have to read the log to notice a remote
    /// that's been unreachable for a while.
    ///
    /// The orchestrator itself doesn't track failure history —
    /// it gets `false` by default. The scheduler decorates the
    /// status before emitting / serving via `get_sync_status`.
    #[serde(default)]
    pub sustained_failure: bool,
    /// Phase §19.10: when set, this device's `meta.json` entry
    /// has been marked stale by the compactor — its
    /// `last_seen_log` was older than the snapshot horizon when
    /// the last sync round opened. Sync rounds short-circuit
    /// while this is latched; the user has to confirm a
    /// snapshot re-pull via the resume dialog before normal
    /// rounds can run again. RFC3339 timestamp of the snapshot
    /// the dialog should reference.
    pub stale_device_since: Option<String>,
}

/// The orchestrator itself. Holds an `Option<adapter>` so the
/// app can start without one configured — `sync_now` returns
/// a sensible "not configured" error in that case rather than
/// panicking.
pub struct SyncOrchestrator {
    db: SharedConn,
    /// `<data_dir>/sync/log/pending/` — the staging directory
    /// the writer drops session files into.
    pending_dir: PathBuf,
    /// `<data_dir>/assets/sounds/` — local store for custom
    /// notification sounds. The §19.10 / §19.11.7 sound-asset
    /// sync hook walks this dir after every successful round to
    /// push local-only files + fetch referenced-but-missing
    /// ones. See `crate::sound_assets`.
    sounds_dir: PathBuf,
    /// Our device id. Used to filter our own files out of
    /// `fetch_new_logs` results so we don't re-apply our own
    /// events.
    local_device_id: DeviceId,
    /// The applier reused across rounds.
    applier: Arc<EventLogApplier>,
    /// Phase Sf: shared with the onboarding command layer. Used
    /// after each successful round to refresh this device's
    /// heartbeat in `meta.json` so other devices and the
    /// compaction algorithm see a current `last_seen_log`.
    onboarding: Arc<OnboardingService>,
    /// Phase Sg: snapshot generator + log compactor. Polled at
    /// the end of every sync round; if the configured thresholds
    /// (age / log-count / byte size since last snapshot) are
    /// breached, a compaction round runs inside the same flow.
    compactor: Arc<Compactor>,
    /// Currently-configured adapter. `None` when the app hasn't
    /// been set up yet.
    adapter: Mutex<Option<Arc<dyn SyncAdapter>>>,
    /// Phase Sl: latched schema-too-old state. Set when a sync
    /// round (or `compatibility_state` probe) encounters a
    /// dataset whose `min_app_version` exceeds our running
    /// build; cleared on the next successful round. Stored as
    /// `Option<String>` carrying the required version so the
    /// status indicator can name it.
    schema_too_old: Mutex<Option<String>>,
    /// §19.10: latched stale-device state. Set when a sync
    /// round notices our `meta.devices[me].stale == true`;
    /// cleared by `resume_from_stale` after the snapshot
    /// re-pull. Carries the snapshot timestamp so the resume
    /// dialog can render it.
    stale_device_since: Mutex<Option<DateTime<Utc>>>,
    /// One-at-a-time guard against overlapping sync rounds.
    /// `try_lock` failure → return early; the user's second
    /// click while a round is in flight produces an
    /// "AlreadyRunning" status instead of starting a parallel
    /// push that would race.
    in_flight: Mutex<bool>,
}

impl SyncOrchestrator {
    pub fn new(
        db: SharedConn,
        pending_dir: PathBuf,
        sounds_dir: PathBuf,
        local_device_id: DeviceId,
        applier: Arc<EventLogApplier>,
        onboarding: Arc<OnboardingService>,
        compactor: Arc<Compactor>,
    ) -> Self {
        Self {
            db,
            pending_dir,
            sounds_dir,
            local_device_id,
            applier,
            onboarding,
            compactor,
            adapter: Mutex::new(None),
            schema_too_old: Mutex::new(None),
            stale_device_since: Mutex::new(None),
            in_flight: Mutex::new(false),
        }
    }

    /// Borrow the compactor handle. Used by the `compact_now`
    /// Tauri command so manual triggers run through the same
    /// instance that the auto-trigger uses.
    pub fn compactor(&self) -> Arc<Compactor> {
        Arc::clone(&self.compactor)
    }

    /// Borrow the currently-configured adapter handle, if any.
    /// Used by the `compact_now` Tauri command so manual
    /// compaction can run against the same adapter the
    /// orchestrator is using, without re-building one from prefs.
    pub fn adapter_handle(&self) -> Option<Arc<dyn SyncAdapter>> {
        self.adapter.lock().expect("adapter mutex poison").clone()
    }

    /// Swap in a freshly-built adapter (the user just configured
    /// or reconfigured the backend). Replacing during a
    /// `sync_now` is safe — the round holds its own `Arc` clone
    /// for the duration.
    pub fn configure(&self, adapter: Arc<dyn SyncAdapter>) {
        let mut guard = self.adapter.lock().expect("adapter mutex poison");
        *guard = Some(adapter);
    }

    /// Tear down the adapter (user picked "Disconnect" in
    /// settings). Subsequent `sync_now` calls return
    /// `SyncStatus::configured = false`.
    pub fn deconfigure(&self) {
        let mut guard = self.adapter.lock().expect("adapter mutex poison");
        *guard = None;
    }

    pub fn status(&self) -> SyncStatus {
        let configured = self
            .adapter
            .lock()
            .expect("adapter mutex poison")
            .is_some();
        let in_flight =
            *self.in_flight.lock().expect("in-flight mutex poison");
        let last_synced_at = self.read_cursor();
        let interval_minutes = read_interval_minutes(&self.db);
        let e2e_enabled = UserPrefsRepo::new(&self.db)
            .get("sync.adapter.e2eEnabled")
            .ok()
            .flatten()
            .as_deref()
            == Some("true");
        let min_app_version_required = self
            .schema_too_old
            .lock()
            .expect("schema_too_old mutex poison")
            .clone();
        let schema_too_old = min_app_version_required.is_some();
        let stale_device_since = self
            .stale_device_since
            .lock()
            .expect("stale_device_since mutex poison")
            .map(|dt| dt.to_rfc3339());
        SyncStatus {
            configured,
            in_flight,
            last_synced_at,
            interval_minutes,
            e2e_enabled,
            schema_too_old,
            min_app_version_required,
            // The orchestrator doesn't track failure history.
            // The scheduler decorates this before emitting and
            // `get_sync_status` does the same when serving the
            // snapshot to the frontend.
            sustained_failure: false,
            stale_device_since,
        }
    }

    /// Borrow the stale-device latch. Used by the resume
    /// command (clears it on a successful re-pull) and by tests
    /// that need to assert the latched state.
    pub fn stale_device_latch(&self) -> Option<DateTime<Utc>> {
        *self
            .stale_device_since
            .lock()
            .expect("stale_device_since mutex poison")
    }

    /// Clear the stale-device latch. Called by the resume
    /// command after a successful snapshot re-pull so the next
    /// sync round can proceed normally.
    pub fn clear_stale_device(&self) {
        *self
            .stale_device_since
            .lock()
            .expect("stale_device_since mutex poison") = None;
    }

    /// Run one sync round. See module docs for the four steps.
    /// Returns Err only on hard failures the user needs to act
    /// on (no adapter configured, adapter `test_connection`
    /// returns Err in step zero). Per-file failures inside the
    /// round downgrade to counters in the report.
    pub async fn sync_now(&self) -> SyncResult<SyncRoundReport> {
        // Take the in-flight guard. Releases on drop so an early
        // `return` past this point still clears it.
        let _guard = InFlightGuard::acquire(&self.in_flight)?;

        let adapter = match self
            .adapter
            .lock()
            .expect("adapter mutex poison")
            .clone()
        {
            Some(a) => a,
            None => {
                return Err(sync_core::SyncError::internal(
                    "no sync adapter configured",
                ));
            }
        };

        // Phase Sl + §19.10: read `meta.json` once and run both
        // gating checks against it.
        //
        // - Schema gate: refuse the round if our running build
        //   is older than `min_app_version`. Sending logs in an
        //   old format to a newer dataset would contaminate it,
        //   and applying newer events into a codebase that
        //   doesn't understand them risks data loss.
        // - Stale gate: refuse the round if our device entry
        //   carries `stale = true`. The compactor has GCed log
        //   files we'd otherwise need to catch up incrementally;
        //   the user has to confirm a snapshot re-pull via the
        //   resume command before normal rounds can resume.
        if let Some(meta) = adapter.fetch_meta().await? {
            // Returns `Err(SchemaTooOld)` when the running version
            // is older than meta.min_app_version.
            match sync_core::ensure_compatible(&meta, self.onboarding.app_version()) {
                Ok(_) => {
                    // Clear any prior latched state — the user
                    // presumably updated since the last failed
                    // round.
                    *self.schema_too_old.lock().expect("mutex poison") = None;
                }
                Err(err) => {
                    // Latch so the status indicator picks it up
                    // until the next successful round.
                    if let sync_core::SyncError::SchemaTooOld { required, .. } = &err {
                        *self.schema_too_old.lock().expect("mutex poison") =
                            Some(required.clone());
                    }
                    return Err(err);
                }
            }
            // §19.10 stale gate. The compactor marks devices
            // whose `last_seen_log` predates the snapshot
            // horizon; we surface that as `StaleDevice` so the
            // frontend can pop the §19.10 resume dialog. The
            // user clicks Fortfahren → `resume_stale_device`
            // command clears the latch + re-pulls the snapshot.
            if let Some(entry) = meta.devices.get(self.local_device_id.as_str())
            {
                if entry.stale {
                    *self
                        .stale_device_since
                        .lock()
                        .expect("stale_device_since mutex poison") =
                        Some(meta.snapshot_timestamp);
                    return Err(sync_core::SyncError::StaleDevice {
                        snapshot_at: meta.snapshot_timestamp.to_rfc3339(),
                    });
                }
            }
        }

        let mut report = SyncRoundReport::default();

        // 1. Push pending logs.
        match self.push_pending(adapter.as_ref()).await {
            Ok(count) => report.pushed_logs = count,
            Err(err) => {
                warn!(?err, "push phase of sync round failed");
                report.push_failures += 1;
            }
        }

        // 2. Fetch + apply.
        let cursor = self.cursor_for_fetch();
        match adapter.fetch_new_logs(&cursor).await {
            Ok(logs) => {
                // Filter out our own device's logs. The remote
                // still has them (the local FS adapter is shared
                // among devices via the same root path) but
                // re-applying our own emissions is wasted work
                // — the applier would just count them as
                // `skipped_own` anyway.
                let foreign: Vec<LogFile> = logs
                    .into_iter()
                    .filter(|log| log.name.device_id != self.local_device_id)
                    .collect();
                report.fetched_logs = foreign.len();

                // Track the newest timestamp we actually saw so
                // the cursor advances even if the apply step
                // partially fails.
                let mut newest = cursor.last_seen_log;
                for log in &foreign {
                    if log.name.timestamp > newest {
                        newest = log.name.timestamp;
                    }
                }

                for log in foreign {
                    match self.applier.apply_log_file(&log) {
                        Ok(apply_report) => report.merge_apply(apply_report),
                        Err(err) => {
                            warn!(
                                log = %log.name.to_filename(),
                                ?err,
                                "apply phase failed for log file",
                            );
                            report.apply_failures += 1;
                        }
                    }
                }

                // 3. Advance cursor. Persist as RFC 3339 to keep
                // the user_prefs value human-readable.
                if newest > cursor.last_seen_log {
                    if let Err(err) = self.save_cursor(newest) {
                        warn!(?err, "couldn't persist sync cursor");
                    }
                }
            }
            Err(err) => {
                warn!(?err, "fetch phase of sync round failed");
                report.push_failures += 1;
            }
        }

        // 4. Heartbeat: refresh our own entry in `meta.json` so
        // other devices see our `last_seen_log` advance. We use
        // `Utc::now()` rather than `newest` because the heartbeat
        // is "this device is alive and current", not "the newest
        // log I observed" — even an empty round still counts.
        //
        // Failures here are non-fatal: the next round retries, and
        // a missed heartbeat at worst means our entry looks
        // slightly stale in someone else's UI until then.
        if let Err(err) = self
            .onboarding
            .heartbeat_meta(adapter.as_ref(), Utc::now())
            .await
        {
            warn!(?err, "meta.json heartbeat failed");
        }

        // 5. (DESIGN.md §19.10 / §19.11.7) Sound-asset sync.
        // Pushes local-only sound files + fetches referenced
        // hashes that aren't present locally. Best-effort: a
        // failure here doesn't sink the round, the next pass
        // retries. The asset bytes flow OUT-OF-BAND from the
        // event log — see `sound_assets` for the algorithm.
        match crate::sound_assets::sync_assets(
            &self.db,
            &self.sounds_dir,
            adapter.as_ref(),
        )
        .await
        {
            Ok(asset_report) => {
                if asset_report.pushed > 0
                    || asset_report.fetched > 0
                    || asset_report.missing_on_remote > 0
                {
                    info!(
                        pushed = asset_report.pushed,
                        fetched = asset_report.fetched,
                        missing = asset_report.missing_on_remote,
                        "sound asset sync",
                    );
                }
            }
            Err(err) => warn!(?err, "sound asset sync failed"),
        }

        // 6. (Phase Sg) Evaluate compaction thresholds. We run
        // inline so the snapshot + log GC happens before the next
        // scheduler tick re-pushes; missing this window once
        // doesn't break correctness, but firing inside the same
        // round lets the user see "compacted" status promptly.
        // Failures are non-fatal — the next round retries.
        match self.compactor.should_compact(adapter.as_ref()).await {
            Ok(true) => {
                info!("compaction thresholds breached; running inline");
                if let Err(err) =
                    self.compactor.compact_now(adapter.as_ref()).await
                {
                    warn!(?err, "auto-compaction failed");
                }
            }
            Ok(false) => {}
            Err(err) => warn!(?err, "couldn't evaluate compaction thresholds"),
        }

        info!(
            pushed = report.pushed_logs,
            fetched = report.fetched_logs,
            applied = report.applied,
            "sync round complete",
        );
        Ok(report)
    }

    /// Push-only variant of [`Self::sync_now`]. Skips the fetch +
    /// apply phases — used by the app-exit hook in `lib.rs` where we
    /// want to flush local mutations to the remote before the
    /// process dies but don't care about pulling new work that
    /// won't be applied before exit anyway.
    ///
    /// Returns the number of files actually pushed. Errors here are
    /// the same "soft" kind `sync_now` produces: a single bad file
    /// downgrades to a warning + counter rather than aborting the
    /// shutdown round.
    pub async fn push_now(&self) -> SyncResult<usize> {
        let _guard = InFlightGuard::acquire(&self.in_flight)?;
        let adapter = match self
            .adapter
            .lock()
            .expect("adapter mutex poison")
            .clone()
        {
            Some(a) => a,
            None => {
                return Err(sync_core::SyncError::internal(
                    "no sync adapter configured",
                ));
            }
        };
        self.push_pending(adapter.as_ref()).await
    }

    /// Walk the pending directory and push every `.jsonl` file
    /// up to the adapter. Returns the number of successful
    /// pushes; per-file errors get a warning + skip rather
    /// than sinking the whole batch.
    ///
    /// We do NOT delete the local file after a successful push.
    /// The writer is still appending to the current-session file
    /// for the rest of this app run, and we'd lose those
    /// additions. Older session files are kept around too — Phase
    /// Sg's compaction handles their eventual GC.
    async fn push_pending(
        &self,
        adapter: &dyn SyncAdapter,
    ) -> SyncResult<usize> {
        let mut entries = match tokio::fs::read_dir(&self.pending_dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Nothing to push — the writer hasn't run yet
                // this session.
                return Ok(0);
            }
            Err(err) => return Err(sync_core::SyncError::io(err.to_string())),
        };

        let mut pushed = 0usize;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| sync_core::SyncError::io(err.to_string()))?
        {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let parsed = match sync_core::LogFileName::from_filename(name) {
                Ok(p) => p,
                Err(_) => {
                    debug!(name = name, "skipping pending entry: not a log file");
                    continue;
                }
            };
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(err) => {
                    warn!(name = name, ?err, "couldn't read pending log");
                    continue;
                }
            };
            let byte_count = bytes.len();
            let log = LogFile {
                name: parsed,
                bytes,
            };
            match adapter.push_log(&log).await {
                Ok(()) => {
                    pushed += 1;
                    // Bump the compactor's "logs since snapshot"
                    // counters so its threshold check picks up the
                    // new push without an extra round-trip.
                    self.compactor.record_pushed_log(byte_count);
                }
                Err(err) => warn!(name = name, ?err, "push_log failed"),
            }
        }
        Ok(pushed)
    }

    fn cursor_for_fetch(&self) -> DeviceCursor {
        let raw = self.read_cursor();
        match raw.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()) {
            Some(ts) => DeviceCursor {
                last_seen_log: ts.with_timezone(&Utc),
            },
            None => DeviceCursor::epoch(),
        }
    }

    fn read_cursor(&self) -> Option<String> {
        UserPrefsRepo::new(&self.db)
            .get(SYNC_CURSOR_PREF_KEY)
            .ok()
            .flatten()
    }

    fn save_cursor(&self, ts: DateTime<Utc>) -> SyncResult<()> {
        UserPrefsRepo::new(&self.db)
            .set(SYNC_CURSOR_PREF_KEY, &ts.to_rfc3339())
            .map_err(|err| {
                sync_core::SyncError::internal(format!(
                    "save cursor: {err}"
                ))
            })?;
        Ok(())
    }
}

/// RAII guard for the in-flight bool. Sets it to `true` on
/// `acquire`; clears it on drop. Acquire returns Err when a
/// round is already in progress — caller surfaces that to the
/// user.
///
/// Uses `std::sync::Mutex<bool>` (not tokio's): the lock is
/// only held during the read-then-write of a bool, never
/// across an `.await`, so a sync mutex is correct + means
/// `Drop` can release without spawning a task.
struct InFlightGuard<'a> {
    flag: &'a Mutex<bool>,
}

impl<'a> InFlightGuard<'a> {
    fn acquire(flag: &'a Mutex<bool>) -> SyncResult<Self> {
        let mut guard = flag.lock().expect("in-flight mutex poison");
        if *guard {
            return Err(sync_core::SyncError::internal(
                "sync already in progress",
            ));
        }
        *guard = true;
        Ok(Self { flag })
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.flag.lock() {
            *guard = false;
        }
        // Poison from a panic mid-round → the bool stays true
        // and the next sync attempt fails with "already in
        // progress". That's the right behaviour given we can't
        // reason about whether the previous round corrupted
        // anything; the user restarts the app.
    }
}
