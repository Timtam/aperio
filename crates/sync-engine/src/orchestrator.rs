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

use async_trait::async_trait;
use chrono::{DateTime, Timelike, Utc};
use sync_core::{DeviceCursor, DeviceId, LogFile, SyncAdapter, SyncError, SyncResult};
use tracing::{debug, info, warn};

use crate::{
    ApplyReport, CompactionReport, Compactor, EventLogApplier, SyncRoundReport, SyncStatus,
    SyncStore, DEFAULT_SYNC_INTERVAL_MINUTES, PREF_SYNC_INTERVAL_MINUTES,
};

/// `user_prefs` key holding the RFC 3339 timestamp of the
/// newest log file we've already fetched from the remote. The
/// orchestrator reads this on entry to filter remote logs and
/// writes it back on success.
pub const SYNC_CURSOR_PREF_KEY: &str = "sync.cursor.lastSeenLog";

/// `user_prefs` key holding the RFC 3339 timestamp of the most
/// recent successful sync round. Distinct from
/// [`SYNC_CURSOR_PREF_KEY`]: that one only advances when foreign
/// logs are actually fetched (because the fetch protocol needs
/// it to skip already-seen files), so on a single-device setup
/// or after a no-op round it never changes. This pref bumps to
/// `Utc::now()` after every successful round so the status
/// panel can show "Letzter Abgleich: vor 2 Min" even when there
/// was nothing to fetch.
pub const SYNC_LAST_ROUND_PREF_KEY: &str = "sync.lastSuccessfulRound";

/// `SyncRoundReport` (what one round did) and `SyncStatus` (the read-only
/// state snapshot) are defined in the crate root so the desktop app and
/// the engine share one definition; this module owns the orchestration
/// that produces them.
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

/// Platform-coordination hooks the sync round invokes but doesn't own —
/// the steps that live outside the reusable engine (DESIGN-sync-engine.md).
/// Desktop wraps the onboarding service (meta heartbeat + app version),
/// the sound-asset sync, and the sync-log audit; mobile provides its own.
/// The two optional steps default to no-ops so a mobile target can adopt
/// them incrementally without the round failing.
#[async_trait]
pub trait SyncRoundHooks: Send + Sync {
    /// This build's app version — fed to the schema-compatibility gate.
    fn app_version(&self) -> String;
    /// Refresh this device's heartbeat in `meta.json` after a round so
    /// other devices + the compactor see a current `last_seen_log`.
    async fn heartbeat(
        &self,
        adapter: &dyn SyncAdapter,
        last_seen_log: DateTime<Utc>,
    ) -> SyncResult<()>;
    /// Sync out-of-band sound assets (push local-only files, fetch
    /// referenced-but-missing ones). Best-effort; default no-op.
    async fn sync_sound_assets(&self, _adapter: &dyn SyncAdapter) -> SyncResult<()> {
        Ok(())
    }
    /// Cache the device names announced in `meta.json` locally so a UI
    /// (the §20.8 Plugins panel's "Used on: <Name>") can render them
    /// without a round-trip. Best-effort; default no-op.
    fn cache_device_names(&self, _meta: &sync_core::MetaJson) {}
    /// Record a compaction outcome in the platform's sync audit log so the
    /// user can see when log files were GCed. Default no-op.
    fn record_compaction(&self, _result: &Result<CompactionReport, SyncError>, _duration_ms: u64) {}
}

/// The orchestrator itself. Holds an `Option<adapter>` so the
/// app can start without one configured — `sync_now` returns
/// a sensible "not configured" error in that case rather than
/// panicking.
pub struct SyncOrchestrator {
    /// Local store seam — the fetch cursor, the last-round timestamp and
    /// the interval pref (all in `user_prefs`).
    store: Arc<dyn SyncStore>,
    /// `<data_dir>/sync/log/pending/` — the staging directory
    /// the writer drops session files into.
    pending_dir: PathBuf,
    /// Our device id. Used to filter our own files out of
    /// `fetch_new_logs` results so we don't re-apply our own
    /// events.
    local_device_id: DeviceId,
    /// The applier reused across rounds.
    applier: Arc<EventLogApplier>,
    /// Platform-coordination hooks: the `meta.json` heartbeat (+ this
    /// build's app version for the schema gate), the out-of-band
    /// sound-asset sync, and the compaction audit log. Desktop wraps the
    /// onboarding service + `sound_assets` + `sync_log`; the round itself
    /// stays platform-agnostic.
    hooks: Arc<dyn SyncRoundHooks>,
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
    /// Timestamp of THIS process launch. Used by the push loop
    /// to tell apart "leftover empty session file from a prior
    /// run" (safe to delete) and "current writer's session file
    /// that just happens to be empty so far" (must keep —
    /// future events in this session would land in it).
    ///
    /// MUST be the exact same instant passed to the
    /// [`EventLogWriter`](crate::EventLogWriter) as its
    /// `session_at` (both are wired from one value in `lib.rs`).
    /// The writer names the live session file with this instant, so
    /// the cleanup's strict `<` keeps it (`==`) while still reaping
    /// genuinely older stubs. If this were minted independently
    /// *after* the writer spawned, the live file's timestamp could
    /// be `< boot_at` and get deleted out from under the open
    /// handle — silent event loss (see `spawn_with_kick`).
    boot_at: DateTime<Utc>,
}

/// Whether an *empty* pending file is a deletable leftover from a
/// prior run rather than the current session's live (still-empty)
/// file. The writer names the live file with `boot_at` — but at
/// SECOND precision (`LogFileName::to_filename` uses
/// `SecondsFormat::Secs`), so the live file always parses back to
/// `boot_at` truncated to the second. We therefore compare at second
/// granularity: a real, sub-second `boot_at` would otherwise make the
/// live file sort `< boot_at` and get reaped — unlinking the writer's
/// open handle on Windows (`FILE_SHARE_DELETE`) and losing the whole
/// session's events. Extracted as a free fn so the invariant is
/// unit-testable without standing up a full orchestrator.
fn is_stale_empty_stub(file_session: DateTime<Utc>, boot_at: DateTime<Utc>) -> bool {
    file_session.with_nanosecond(0).unwrap_or(file_session)
        < boot_at.with_nanosecond(0).unwrap_or(boot_at)
}

/// True when a sync round must RESUME via snapshot rather than fetch-and-replay:
/// the device's fetch `cursor` predates the dataset's `snapshot_ts` horizon, so
/// the compactor may have GC'd the pre-snapshot logs the device never consumed.
/// A cursor at or after the horizon is caught up (or freshly resumed) and passes
/// — so the §19.10 coverage gate fires at most once per device per snapshot.
/// Free fn so the boundary is unit-testable without a full orchestrator.
fn needs_snapshot_resume(cursor: DateTime<Utc>, snapshot_ts: DateTime<Utc>) -> bool {
    cursor < snapshot_ts
}

impl SyncOrchestrator {
    pub fn new(
        store: Arc<dyn SyncStore>,
        pending_dir: PathBuf,
        local_device_id: DeviceId,
        applier: Arc<EventLogApplier>,
        hooks: Arc<dyn SyncRoundHooks>,
        compactor: Arc<Compactor>,
        boot_at: DateTime<Utc>,
    ) -> Self {
        Self {
            store,
            pending_dir,
            local_device_id,
            applier,
            hooks,
            compactor,
            adapter: Mutex::new(None),
            schema_too_old: Mutex::new(None),
            stale_device_since: Mutex::new(None),
            in_flight: Mutex::new(false),
            boot_at,
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
        let configured = self.adapter.lock().expect("adapter mutex poison").is_some();
        let in_flight = *self.in_flight.lock().expect("in-flight mutex poison");
        let last_synced_at = self
            .store
            .get_pref(SYNC_LAST_ROUND_PREF_KEY)
            .ok()
            .flatten()
            // Fall back to the fetch cursor on pre-upgrade
            // datasets that don't have the new pref written yet.
            .or_else(|| self.read_cursor());
        let interval_minutes = self.read_interval_minutes();
        let e2e_enabled = self
            .store
            .get_pref("sync.adapter.e2eEnabled")
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
            // Same pattern as `sustained_failure`: the
            // orchestrator returns `None`; the scheduler
            // decorates this with whatever it last latched
            // from a failed round.
            last_error_code: None,
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

        let adapter = match self.adapter.lock().expect("adapter mutex poison").clone() {
            Some(a) => a,
            None => {
                return Err(sync_core::SyncError::internal("no sync adapter configured"));
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
            match sync_core::ensure_compatible(&meta, &self.hooks.app_version()) {
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
                        *self.schema_too_old.lock().expect("mutex poison") = Some(required.clone());
                    }
                    return Err(err);
                }
            }
            // §20.8 helper: cache every announced device's name into the
            // local device_names table so the Settings → Plugins panel can
            // render "Used on: <Name>" without a separate round-trip. A
            // platform hook owns the table; best-effort, errors swallowed.
            self.hooks.cache_device_names(&meta);

            // §19.10 stale gate. The compactor marks devices
            // whose `last_seen_log` predates the snapshot
            // horizon; we surface that as `StaleDevice` so the
            // frontend can pop the §19.10 resume dialog. The
            // user clicks Fortfahren → `resume_stale_device`
            // command clears the latch + re-pulls the snapshot.
            if let Some(entry) = meta.devices.get(self.local_device_id.as_str()) {
                if entry.stale {
                    *self
                        .stale_device_since
                        .lock()
                        .expect("stale_device_since mutex poison") = Some(meta.snapshot_timestamp);
                    return Err(sync_core::SyncError::StaleDevice {
                        snapshot_at: meta.snapshot_timestamp.to_rfc3339(),
                    });
                }
            }

            // §19.10 coverage backstop. The compactor GCs every log older than
            // the snapshot horizon, so a device whose fetch cursor still
            // predates that horizon would silently SKIP the deleted range and
            // lose those events. The per-device `stale` flag above is NOT enough
            // on its own: a behind device can heartbeat itself non-stale (the
            // heartbeat stamps `now` even when its fetch/apply failed), be absent
            // from `meta.devices` (the compactor can't flag a device it can't
            // see), or have its flag clobbered by a concurrent compactor's
            // last-write-wins meta push. So gate on the REAL cursor instead: any
            // sub-snapshot cursor means "consume the snapshot, don't replay" —
            // which `resume_from_stale` does, advancing the cursor to the
            // snapshot horizon. A cursor already at/after the horizon (caught up,
            // or freshly resumed) passes, so this fires at most once per device
            // per snapshot, exactly like the §19.10 gate it backstops.
            if needs_snapshot_resume(
                self.cursor_for_fetch().last_seen_log,
                meta.snapshot_timestamp,
            ) {
                *self
                    .stale_device_since
                    .lock()
                    .expect("stale_device_since mutex poison") = Some(meta.snapshot_timestamp);
                return Err(sync_core::SyncError::StaleDevice {
                    snapshot_at: meta.snapshot_timestamp.to_rfc3339(),
                });
            }

            // §19.7 encryption gate. The dataset is end-to-end encrypted but
            // this device isn't in E2E mode — i.e. another device flipped
            // encryption on after we onboarded in plaintext. Refuse the round
            // BEFORE the push step: our adapter is plain, so pushing would
            // write readable logs into the encrypted dataset (corrupting it /
            // leaking plaintext), and fetched logs are ciphertext we can't
            // apply. `EncryptionRequired` latches as `last_error_code =
            // encryption_required`, which the frontend turns into the §19.7
            // "enter the dataset passphrase to adopt encryption" prompt
            // (desktop `adopt_remote_encryption`). The pref is the source of
            // truth for "am I encrypting"; it flips true the moment this device
            // enables or adopts E2E, clearing this gate.
            if meta.e2e_enabled && !self.store.e2e_enabled() {
                return Err(sync_core::SyncError::EncryptionRequired);
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
        if let Err(err) = self.hooks.heartbeat(adapter.as_ref(), Utc::now()).await {
            warn!(?err, "meta.json heartbeat failed");
        }

        // 5. (DESIGN.md §19.10 / §19.11.7) Sound-asset sync — pushes
        // local-only sound files + fetches referenced-but-missing hashes,
        // out-of-band from the event log. A platform hook owns the
        // algorithm (desktop: `sound_assets`). Best-effort: a failure here
        // doesn't sink the round, the next pass retries.
        if let Err(err) = self.hooks.sync_sound_assets(adapter.as_ref()).await {
            warn!(?err, "sound asset sync failed");
        }

        // 6. (Phase Sg) Evaluate compaction thresholds. We run
        // inline so the snapshot + log GC happens before the next
        // scheduler tick re-pushes; missing this window once
        // doesn't break correctness, but firing inside the same
        // round lets the user see "compacted" status promptly.
        // Failures are non-fatal — the next round retries.
        //
        // §19.10 — record the outcome in the Protokoll so the user
        // can see when log files were GCed. Manual `compact_now`
        // logs via the scheduler; the auto path here writes
        // directly via `SyncLogRepo` since the orchestrator
        // doesn't hold a scheduler reference (the relationship
        // goes the other way).
        match self.compactor.should_compact(adapter.as_ref()).await {
            Ok(true) => {
                info!("compaction thresholds breached; running inline");
                let started = std::time::Instant::now();
                let outcome = self.compactor.compact_now(adapter.as_ref()).await;
                let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                if let Err(err) = &outcome {
                    warn!(?err, "auto-compaction failed");
                }
                self.hooks.record_compaction(&outcome, duration_ms);
            }
            Ok(false) => {}
            Err(err) => warn!(?err, "couldn't evaluate compaction thresholds"),
        }

        // Record that the round finished successfully. The UI's
        // "last synced at" reads this pref, not the fetch cursor
        // — the cursor only advances when we actually fetch
        // foreign logs, so on a single-device setup it would
        // never move and the user would forever see "noch kein
        // Abgleich" even after dozens of successful rounds.
        if let Err(err) = self
            .store
            .set_pref(SYNC_LAST_ROUND_PREF_KEY, &Utc::now().to_rfc3339())
        {
            warn!(?err, "couldn't persist last-round timestamp");
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
        let adapter = match self.adapter.lock().expect("adapter mutex poison").clone() {
            Some(a) => a,
            None => {
                return Err(sync_core::SyncError::internal("no sync adapter configured"));
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
    async fn push_pending(&self, adapter: &dyn SyncAdapter) -> SyncResult<usize> {
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
            // The EventLogWriter pre-creates a session file at
            // app start, before knowing whether the session will
            // produce any events. If it doesn't (e.g. the user
            // opens Aperio, browses, closes), we end up with a
            // 0-byte file in `pending/`. Pushing those would
            // clutter the remote sync folder with empty
            // placeholders — skip + delete the local stub.
            // We can only safely delete the file if the writer
            // for THIS session has already rotated away from it.
            // Cheap check: if the timestamp in the filename is
            // older than this app launch, the writer can't be
            // appending to it. We use the parsed timestamp
            // directly; the writer mints a fresh name per
            // session so this never races with the active
            // session file.
            if bytes.is_empty() {
                if is_stale_empty_stub(parsed.timestamp, self.boot_at) {
                    if let Err(err) = tokio::fs::remove_file(&path).await {
                        debug!(name = name, ?err, "couldn't remove empty pending log",);
                    } else {
                        debug!(name = name, "skipped + removed empty pending log");
                    }
                } else {
                    debug!(name = name, "skipping empty pending log (current session)",);
                }
                continue;
            }
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
        self.store.get_pref(SYNC_CURSOR_PREF_KEY).ok().flatten()
    }

    fn save_cursor(&self, ts: DateTime<Utc>) -> SyncResult<()> {
        self.store
            .set_pref(SYNC_CURSOR_PREF_KEY, &ts.to_rfc3339())
            .map_err(|err| sync_core::SyncError::internal(format!("save cursor: {err}")))?;
        Ok(())
    }

    /// The configured periodic sync interval (minutes), read from
    /// `user_prefs` with the default fallback. Mirrors the desktop
    /// scheduler's `read_interval_minutes`; surfaced in `SyncStatus`.
    fn read_interval_minutes(&self) -> u32 {
        self.store
            .get_pref(PREF_SYNC_INTERVAL_MINUTES)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            // Clamp to >= 1 minute, matching the desktop scheduler's
            // `read_interval_minutes` so a stray "0" can't busy-loop.
            .map(|m| m.max(1))
            .unwrap_or(DEFAULT_SYNC_INTERVAL_MINUTES)
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
            return Err(sync_core::SyncError::internal("sync already in progress"));
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

#[cfg(test)]
mod tests {
    use super::{is_stale_empty_stub, needs_snapshot_resume};
    use chrono::{Duration, TimeZone, Timelike, Utc};

    #[test]
    fn needs_snapshot_resume_boundary() {
        let snap = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        // Cursor BEFORE the snapshot horizon → must resume (pre-snapshot logs
        // may have been GC'd by the compactor; replaying would skip them).
        assert!(needs_snapshot_resume(snap - Duration::seconds(1), snap));
        // Cursor EXACTLY at the horizon (caught up / freshly resumed) → pass, so
        // the gate can't latch forever after a resume advances cursor = horizon.
        assert!(!needs_snapshot_resume(snap, snap));
        // Cursor AHEAD of the horizon → pass.
        assert!(!needs_snapshot_resume(snap + Duration::seconds(1), snap));
    }

    #[test]
    fn empty_stub_cleanup_keeps_live_session_file_and_reaps_older() {
        let boot = Utc.with_ymd_and_hms(2026, 5, 31, 7, 0, 0).unwrap();

        // The writer names the CURRENT session's file with exactly
        // `boot_at`. It must NOT be classed as a deletable stub —
        // otherwise the still-empty live file gets unlinked out from
        // under the open handle (silent event loss on Windows).
        assert!(
            !is_stale_empty_stub(boot, boot),
            "live session file (timestamp == boot_at) must be kept",
        );

        // A genuinely older empty file (a prior run that produced no
        // events) is a reapable leftover stub.
        assert!(
            is_stale_empty_stub(boot - Duration::seconds(1), boot),
            "an empty file from before this launch must be deletable",
        );

        // Defensive: a (clock-skew) later timestamp is also kept.
        assert!(!is_stale_empty_stub(boot + Duration::seconds(1), boot));
    }

    #[test]
    fn sub_second_boot_at_keeps_the_second_granular_live_file() {
        // Real boot_at carries sub-seconds, but the writer's filename is
        // second-granular (`LogFileName::to_filename`), so the live file
        // parses back to boot_at truncated to the whole second. The
        // cleanup must compare at second granularity — otherwise the
        // live file (boot_at truncated to .000) sorts `< boot_at`
        // (…00.523) and gets reaped: the Windows ghost-file event-loss
        // bug. This case is what the whole-second test above missed.
        let boot = Utc
            .with_ymd_and_hms(2026, 5, 31, 7, 0, 0)
            .unwrap()
            .with_nanosecond(523_000_000)
            .unwrap(); // 07:00:00.523
        let live = Utc.with_ymd_and_hms(2026, 5, 31, 7, 0, 0).unwrap(); // filename → .000
        assert!(
            !is_stale_empty_stub(live, boot),
            "the live session file must survive a sub-second boot_at",
        );
        // A genuinely older session (a prior whole second) is still reaped.
        let older = Utc.with_ymd_and_hms(2026, 5, 31, 6, 59, 59).unwrap();
        assert!(is_stale_empty_stub(older, boot));
    }
}
