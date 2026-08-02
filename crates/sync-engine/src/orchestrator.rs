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

/// `user_prefs` key holding the RFC 3339 timestamp of the newest log
/// file THIS device has written (its own session files). Distinct from
/// [`SYNC_CURSOR_PREF_KEY`], which only tracks *foreign* logs: own logs
/// are filtered out before the cursor advances, so the cursor
/// structurally sits below this device's own newest log.
///
/// Together they give the device's TRUE held horizon —
/// `max(cursor, own_newest_log)` — the point up to which it holds every
/// event (foreign-applied + own-written). The §19.10 stale backstop and
/// the compactor's snapshot horizon both reason in terms of this held
/// horizon, so a caught-up device whose own logs are the newest in the
/// dataset isn't mistaken for one that fell behind the snapshot.
pub const SYNC_OWN_NEWEST_LOG_PREF_KEY: &str = "sync.cursor.ownNewestLog";

/// JSON map (filename → applied byte length) of FOREIGN log files this
/// device has fetched and applied — the growth-refetch signal
/// ([`DeviceCursor::known_lengths`]): a peer's live session file that
/// gained appended events is re-fetched when its listed size exceeds the
/// recorded applied length, instead of those events staying invisible
/// until the peer rotates its session. Persisted (not in-memory) so
/// appends that landed while this device was OFFLINE are detected on the
/// next launch. Capped to the newest [`APPLIED_LOG_LENGTHS_CAP`] files.
pub const SYNC_APPLIED_LOG_LENGTHS_PREF_KEY: &str = "sync.cursor.appliedLogLengths";

/// Cap for [`SYNC_APPLIED_LOG_LENGTHS_PREF_KEY`]: live session files are
/// few (one per device until compaction sweeps), so the newest N entries
/// by embedded filename timestamp comfortably cover every file that can
/// still grow while keeping the pref bounded.
pub const APPLIED_LOG_LENGTHS_CAP: usize = 64;

/// Merge freshly applied `(filename, byte length)` records into the
/// serialized map [`SYNC_APPLIED_LOG_LENGTHS_PREF_KEY`] holds, applying
/// the cap (filenames sort chronologically — the timestamp prefix is
/// fixed-width — so a plain string sort ages out swept files). Returns
/// the new serialized map, or `None` when there is nothing to record.
/// Pure, so the onboarding/stale-resume paths — which fetch + apply logs
/// OUTSIDE the round — share the exact recording semantics; without
/// that, a peer's live session file applied during onboarding had no
/// length entry and its later appends stayed invisible until rotation
/// (the append-miss this mechanism exists to fix).
pub fn merge_applied_log_lengths(
    existing_raw: Option<&str>,
    fetched: &[(String, u64)],
) -> Option<String> {
    if fetched.is_empty() {
        return None;
    }
    let mut map: std::collections::HashMap<String, u64> = existing_raw
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    for (name, len) in fetched {
        map.insert(name.clone(), *len);
    }
    if map.len() > APPLIED_LOG_LENGTHS_CAP {
        let mut names: Vec<String> = map.keys().cloned().collect();
        names.sort_unstable();
        let drop_count = names.len() - APPLIED_LOG_LENGTHS_CAP;
        for name in names.into_iter().take(drop_count) {
            map.remove(&name);
        }
    }
    serde_json::to_string(&map).ok()
}

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
    /// `round_meta` is the meta the round already fetched for its gates
    /// (None when the remote had none, or when the round invalidated its
    /// copy) — implementations use it instead of re-fetching, and may skip
    /// the push entirely when this device's record is already current.
    /// §19.5's last-write-wins tolerates the copy being seconds old.
    async fn heartbeat(
        &self,
        adapter: &dyn SyncAdapter,
        last_seen_log: DateTime<Utc>,
        round_meta: Option<&sync_core::MetaJson>,
    ) -> SyncResult<()>;
    /// Sync out-of-band sound assets (push local-only files, fetch
    /// referenced-but-missing ones). Best-effort; default no-op.
    async fn sync_sound_assets(&self, _adapter: &dyn SyncAdapter) -> SyncResult<()> {
        Ok(())
    }
    /// §19.10 — recover a device that fell behind the GC horizon: re-pull the
    /// current `snapshot.json`, apply it, replay this device's own pending logs
    /// over it (preserving offline edits), and clear its `stale` flag in
    /// `meta.json`. Invoked AUTOMATICALLY by the round the moment the device is
    /// detected stale, so the user never has to confirm — the platform wraps
    /// its onboarding service's `resume_from_stale`.
    ///
    /// The default errors: a platform that doesn't wire this up falls back to
    /// the old behaviour (the round surfaces `StaleDevice` and the manual
    /// resume dialog handles it).
    async fn resume_from_stale(&self, _adapter: &dyn SyncAdapter) -> SyncResult<()> {
        Err(SyncError::internal(
            "auto-resume not supported on this platform",
        ))
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
    /// Byte length last successfully PUSHED per pending filename, so a
    /// round skips re-uploading a session file that hasn't grown since
    /// (the writer only appends, so length equality means content
    /// equality). Deliberately IN-MEMORY: every file is re-pushed at
    /// least once per app session, which self-heals a remote copy that
    /// was manually deleted or truncated behind our back.
    pushed_lengths: Mutex<std::collections::HashMap<String, u64>>,
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

/// Whether the §19.10 held-horizon backstop must force a snapshot resume:
/// this device's `held` horizon — `max(foreign cursor, own newest log)` — is
/// below the dataset's GC high-water mark, i.e. the compactor has DELETED
/// pre-snapshot logs this device never consumed, so it can't catch up
/// incrementally and must consume the snapshot.
///
/// Keying on `gc_horizon` rather than `snapshot_timestamp` is deliberate on two
/// counts: (1) a device merely behind the snapshot but at/above the GC mark can
/// still replay the RETAINED logs — flagging it would be a spurious resume
/// (the over-fire); (2) `gc_horizon` is `None` (→ `MIN_UTC`) on any dataset
/// that has never GC'd a log, including a LEGACY meta that carries a real
/// `now()`-baseline `snapshot_timestamp` but has no `snapshot.json`. Gating on
/// `snapshot_timestamp` there wedged such datasets in an endless resume loop;
/// gating on `gc_horizon` can't, because `held >= MIN_UTC` always. Free fn so
/// the boundary is unit-testable without standing up a full orchestrator.
fn snapshot_backstop_trips(held: DateTime<Utc>, meta: &sync_core::MetaJson) -> bool {
    held < meta.gc_horizon_or_min()
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
            pushed_lengths: Mutex::new(std::collections::HashMap::new()),
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
        // A (re)configured backend may point at a DIFFERENT remote that
        // doesn't hold our files — forget the per-session pushed lengths
        // so the next round re-pushes everything once.
        self.pushed_lengths
            .lock()
            .expect("pushed_lengths mutex poison")
            .clear();
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
            .or_else(|| self.read_cursor().ok().flatten());
        let interval_minutes = self.read_interval_minutes();
        let e2e_enabled = self
            .store
            .get_pref(crate::whitelist::PREF_E2E_ENABLED)
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

        // Phase Sl + §19.10: read `meta.json` ONCE for the whole round.
        // The gates below consume it, and the same copy is threaded into
        // the heartbeat + compaction-threshold steps at the end — those
        // used to each re-fetch it, making meta.json 3 GETs of a no-change
        // round's ~5 serial requests (§ the WebDAV round-trip audit).
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
        let mut round_meta = adapter.fetch_meta().await?;
        // Set when the §19.10 auto-resume ran: it pushes an UPDATED meta
        // (stale flag cleared), so the round's cached copy is invalid from
        // then on — the heartbeat must re-fetch or it would push the old
        // copy back, re-flagging this device as stale.
        let mut resumed = false;
        if let Some(meta) = &round_meta {
            // Returns `Err(SchemaTooOld)` when the running version
            // is older than meta.min_app_version.
            match sync_core::ensure_compatible(meta, &self.hooks.app_version()) {
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
            self.hooks.cache_device_names(meta);

            // §19.10 stale detection + AUTO-RESUME. A device is stale when
            // either the compactor flagged its `meta.devices[me].stale`, OR our
            // own held-horizon backstop trips — `max(foreign cursor, own newest
            // log)` below the dataset's GC high-water mark (`gc_horizon`),
            // meaning the compactor DELETED pre-snapshot logs we never consumed.
            //
            // Why two signals: the per-device flag can miss (we may be absent
            // from `meta.devices`, or a concurrent compactor's last-write-wins
            // meta push clobbers the flag), and the backstop keys on
            // `gc_horizon` not `snapshot_timestamp` so a device merely behind
            // the snapshot but still able to replay the RETAINED logs isn't
            // flagged, and a never-GC'd dataset (`gc_horizon == None → MIN_UTC`)
            // never trips (a fresh OR legacy single-device dataset can't wedge).
            // The HELD horizon (not the bare foreign cursor) keeps a caught-up
            // device whose own logs are the newest from being flagged.
            //
            // Rather than bail with `StaleDevice` and make the user click
            // Fortfahren, we AUTO-RESUME inline: re-pull the snapshot, replay
            // our own pending logs over it (offline edits preserved), clear the
            // flag, then continue the round normally. The user never sees a
            // "device offline too long" prompt. Only when the auto-resume
            // FAILS (e.g. offline, or an unsupported platform) do we latch +
            // surface `StaleDevice`, so the next round retries and the manual
            // resume dialog stays as a fallback.
            let flagged_stale = meta
                .devices
                .get(self.local_device_id.as_str())
                .is_some_and(|entry| entry.stale);
            // An unreadable horizon must not decide this. Flooring would make
            // the backstop trip and re-onboard the device; treating it as "not
            // stale" leaves the flag from meta.json, which the peer computed
            // from what this device last published.
            let backstop = match self.held_horizon() {
                Ok(horizon) => snapshot_backstop_trips(horizon, meta),
                Err(err) => {
                    warn!(
                        ?err,
                        "couldn't read this device's horizon; not tripping the backstop"
                    );
                    false
                }
            };
            if flagged_stale || backstop {
                match self.hooks.resume_from_stale(adapter.as_ref()).await {
                    Ok(()) => {
                        info!("§19.10: device was stale; auto-resumed via snapshot re-pull");
                        // Drop any prior latch — the resume cleared the flag,
                        // advanced our cursor past `gc_horizon`, and pushed an
                        // updated meta, so the rest of the round proceeds as a
                        // caught-up device.
                        self.clear_stale_device();
                        resumed = true;
                    }
                    Err(err) => {
                        warn!(
                            ?err,
                            "§19.10 auto-resume failed; surfacing StaleDevice for retry",
                        );
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
        if resumed {
            round_meta = None;
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
        let cursor = self.cursor_for_fetch()?;
        match adapter.fetch_new_logs(&cursor).await {
            Ok(logs) => {
                // Filter out our own device's logs. The remote
                // still has them (the local FS adapter is shared
                // among devices via the same root path) but
                // re-applying our own emissions is wasted work
                // — the applier would just count them as
                // `skipped_own` anyway.
                let mut foreign: Vec<LogFile> = logs
                    .into_iter()
                    .filter(|log| log.name.device_id != self.local_device_id)
                    .collect();
                // Apply in CHRONOLOGICAL order regardless of what order
                // the adapter returned (the trait explicitly allows
                // unordered results, and the WebDAV adapter's concurrent
                // GETs yield in completion order). The applier only
                // orders envelopes WITHIN one file; across files,
                // creates are unconditional upserts and deletes are
                // point lookups — applying a rotated-away CREATE after
                // its later DELETE would resurrect the item.
                foreign.sort_by(|a, b| {
                    a.name
                        .timestamp
                        .cmp(&b.name.timestamp)
                        .then_with(|| a.name.device_id.as_str().cmp(b.name.device_id.as_str()))
                });
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

                // Record each applied file's byte length — the
                // growth-refetch signal for the next round. A FAILED
                // apply (e.g. a torn read of a live file's last line
                // fails the whole file) records length 0: the cursor
                // still advances past the file, so without a record it
                // would never be looked at again — with 0, any listed
                // size counts as grown and a size-reporting adapter
                // retries it next round.
                let mut applied_lengths: Vec<(String, u64)> = Vec::new();
                for log in foreign {
                    let byte_len = log.bytes.len() as u64;
                    match self.applier.apply_log_file(&log) {
                        Ok(apply_report) => {
                            report.merge_apply(apply_report);
                            applied_lengths.push((log.name.to_filename(), byte_len));
                        }
                        Err(err) => {
                            warn!(
                                log = %log.name.to_filename(),
                                ?err,
                                "apply phase failed for log file",
                            );
                            report.apply_failures += 1;
                            applied_lengths.push((log.name.to_filename(), 0));
                        }
                    }
                }
                self.record_applied_lengths(&applied_lengths);

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
                report.fetch_failures += 1;
            }
        }

        // 4. Heartbeat: refresh our own `last_seen_log` in `meta.json`.
        // We stamp our HELD HORIZON — `max(cursor, own_newest_log)` — not
        // `Utc::now()`. `last_seen_log` is the compactor's input for
        // deciding which devices fell behind the snapshot (and which logs
        // are GC-safe), so it must mean "the point up to which I actually
        // hold every event", not "I'm alive". A wall-clock heartbeat
        // decoupled the two: a device whose fetch failed still stamped
        // `now`, so the compactor judged it caught up and GC'd logs it
        // never consumed. (Liveness — the "Letzter Abgleich: vor 2 Min"
        // display — comes from `SYNC_LAST_ROUND_PREF_KEY` below, which DOES
        // bump every round.)
        //
        // Failures here are non-fatal: the next round retries, and a missed
        // heartbeat at worst means our entry looks slightly behind in
        // someone else's UI until then.
        //
        // Skipped entirely when the horizon cannot be read, rather than
        // published as the `MIN_UTC` floor. See `held_horizon` — publishing "I
        // have nothing" is what gets a device flagged stale and re-onboarded.
        match self.held_horizon() {
            Ok(horizon) => {
                if let Err(err) = self
                    .hooks
                    .heartbeat(adapter.as_ref(), horizon, round_meta.as_ref())
                    .await
                {
                    warn!(?err, "meta.json heartbeat failed");
                }
            }
            Err(err) => warn!(?err, "skipping the heartbeat: horizon unreadable"),
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
        match self
            .compactor
            .should_compact(adapter.as_ref(), round_meta.as_ref())
            .await
        {
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
        // Track the newest own-written session file we see so the held
        // horizon (max(cursor, own_newest)) stays current. We record EVERY
        // own session file's timestamp — even an empty current-session stub
        // we skip pushing below — because the file's mere existence means
        // this device holds events up to that session's start.
        let mut newest_own: Option<DateTime<Utc>> = None;
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
            if parsed.device_id == self.local_device_id {
                newest_own =
                    Some(newest_own.map_or(parsed.timestamp, |cur| cur.max(parsed.timestamp)));
            }
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
            // Skip a file whose byte length matches what this SESSION
            // already pushed — the writer only appends, so equal length
            // means identical content, and re-uploading the whole (all-day
            // growing) session file every round was often the slowest
            // request of the round. The map is in-memory by design: a
            // fresh app session re-pushes everything once, which
            // self-heals a remote copy deleted/truncated behind our back.
            let already_pushed = self
                .pushed_lengths
                .lock()
                .expect("pushed_lengths mutex poison")
                .get(name)
                .is_some_and(|len| *len == byte_count as u64);
            if already_pushed {
                debug!(
                    name = name,
                    "pending log unchanged since last push; skipping"
                );
                continue;
            }
            let log = LogFile {
                name: parsed,
                bytes,
            };
            match adapter.push_log(&log).await {
                Ok(()) => {
                    pushed += 1;
                    self.pushed_lengths
                        .lock()
                        .expect("pushed_lengths mutex poison")
                        .insert(name.to_string(), byte_count as u64);
                    // Bump the compactor's "logs since snapshot"
                    // counters so its threshold check picks up the
                    // new push without an extra round-trip.
                    self.compactor.record_pushed_log(byte_count);
                }
                Err(err) => warn!(name = name, ?err, "push_log failed"),
            }
        }
        // Persist the newest own-written session timestamp so the held
        // horizon and the compactor's content-bounded snapshot timestamp
        // can read it without re-scanning the pending dir. Monotonic: only
        // advance, never regress (a swept/rotated-away file mustn't lower it).
        if let Some(ts) = newest_own {
            // Monotonic only against a horizon we could actually read. An
            // unreadable one would compare against `MIN_UTC` and let a swept
            // file lower the stored value.
            if self.read_own_newest_log().is_ok_and(|held| ts > held) {
                if let Err(err) = self
                    .store
                    .set_pref(SYNC_OWN_NEWEST_LOG_PREF_KEY, &ts.to_rfc3339())
                {
                    warn!(?err, "couldn't persist own-newest-log horizon");
                }
            }
        }
        Ok(pushed)
    }

    /// This device's held horizon: the newest point up to which it holds
    /// every event, foreign-applied OR own-written. `max(cursor,
    /// own_newest_log)`. The §19.10 stale backstop compares this against the
    /// dataset's snapshot horizon — a device is behind only when BOTH its
    /// foreign cursor and its own newest log predate the snapshot.
    ///
    /// Fails rather than flooring when either half cannot be read. This value
    /// is PUBLISHED — it is what tells every other device how far this one has
    /// got. `MIN_UTC` published as a held horizon says "I have nothing", so the
    /// next peer to compact flags this device stale, and its next round is a
    /// forced re-onboard through a full snapshot pull. A momentary lock is not
    /// worth that. Saying nothing leaves the previous heartbeat standing, which
    /// is merely slightly stale and self-corrects next round.
    fn held_horizon(&self) -> SyncResult<DateTime<Utc>> {
        Ok(self
            .cursor_for_fetch()?
            .last_seen_log
            .max(self.read_own_newest_log()?))
    }

    /// Read the persisted newest own-written log timestamp, or `MIN_UTC`
    /// when this device has never written one.
    /// A missing pref floors to `MIN_UTC` — this device has genuinely written
    /// nothing. A failed READ does not: see [`Self::held_horizon`]. An
    /// unparseable value still floors, because a garbled timestamp is a value
    /// this device wrote and cannot use, not a store that would not answer.
    fn read_own_newest_log(&self) -> SyncResult<DateTime<Utc>> {
        let raw = self
            .store
            .get_pref(SYNC_OWN_NEWEST_LOG_PREF_KEY)
            .map_err(|err| {
                sync_core::SyncError::internal(format!(
                    "read {SYNC_OWN_NEWEST_LOG_PREF_KEY}: {err}"
                ))
            })?;
        Ok(raw
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::MIN_UTC))
    }

    fn cursor_for_fetch(&self) -> SyncResult<DeviceCursor> {
        let raw = self.read_cursor()?;
        // Exclude our own device's files at the LISTING stage: they sit
        // above the cursor (it only advances on foreign logs) and were
        // re-downloaded in full every round just for the post-fetch
        // filter below to discard them. The compactor's GC scan builds
        // its own epoch cursor WITHOUT the exclusion — it genuinely
        // needs own files for coverage decisions.
        let exclude_device = Some(self.local_device_id.clone());
        // Applied byte lengths → the adapter's growth-refetch signal
        // (a peer's live session file gaining appended events).
        let known_lengths = self
            .read_applied_lengths()
            .into_iter()
            .map(|(name, len)| sync_core::KnownLogLength { name, len })
            .collect();
        Ok(
            match raw.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()) {
                Some(ts) => DeviceCursor {
                    last_seen_log: ts.with_timezone(&Utc),
                    exclude_device,
                    known_lengths,
                },
                // Absent or unparseable: start from the epoch and re-apply. The
                // apply side is idempotent, so this costs bandwidth, not
                // correctness — which is why it stays a fallback while an
                // unreadable STORE does not.
                None => DeviceCursor {
                    exclude_device,
                    known_lengths,
                    ..DeviceCursor::epoch()
                },
            },
        )
    }

    /// The persisted (filename → applied byte length) map backing
    /// [`SYNC_APPLIED_LOG_LENGTHS_PREF_KEY`]. Unreadable/corrupt prefs
    /// degrade to empty — the only cost is a one-time re-fetch.
    fn read_applied_lengths(&self) -> std::collections::HashMap<String, u64> {
        self.store
            .get_pref(SYNC_APPLIED_LOG_LENGTHS_PREF_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Record the applied byte lengths of freshly fetched foreign logs
    /// (see [`merge_applied_log_lengths`]).
    fn record_applied_lengths(&self, fetched: &[(String, u64)]) {
        let existing = self
            .store
            .get_pref(SYNC_APPLIED_LOG_LENGTHS_PREF_KEY)
            .ok()
            .flatten();
        if let Some(raw) = merge_applied_log_lengths(existing.as_deref(), fetched) {
            if let Err(err) = self.store.set_pref(SYNC_APPLIED_LOG_LENGTHS_PREF_KEY, &raw) {
                warn!(?err, "couldn't persist applied-log-length map");
            }
        }
    }

    fn read_cursor(&self) -> SyncResult<Option<String>> {
        self.store.get_pref(SYNC_CURSOR_PREF_KEY).map_err(|err| {
            sync_core::SyncError::internal(format!("read {SYNC_CURSOR_PREF_KEY}: {err}"))
        })
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
    use super::{is_stale_empty_stub, snapshot_backstop_trips};
    use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
    use sync_core::{DeviceId, DeviceRecord, MetaJson};

    /// Build a meta whose GC high-water mark sits at `gc_horizon` — i.e. logs
    /// below it have been deleted from the remote.
    fn meta_with_gc_horizon(gc_horizon: DateTime<Utc>) -> MetaJson {
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.gc_horizon = Some(gc_horizon);
        meta
    }

    #[test]
    fn backstop_keys_on_the_gc_horizon_not_the_snapshot() {
        let gc = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let meta = meta_with_gc_horizon(gc);
        // Held horizon BELOW the GC mark → must resume: the logs needed to
        // catch up incrementally have been deleted.
        assert!(snapshot_backstop_trips(gc - Duration::seconds(1), &meta));
        // Held horizon EXACTLY at the mark (a caught-up device) → pass.
        assert!(!snapshot_backstop_trips(gc, &meta));
        // Held horizon AHEAD of the mark → pass.
        assert!(!snapshot_backstop_trips(gc + Duration::seconds(1), &meta));
    }

    #[test]
    fn backstop_does_not_trip_on_a_behind_device_above_the_gc_horizon() {
        // The over-fire guard: a snapshot exists and the device is BEHIND it,
        // but it sits at/above the GC mark — the retained logs let it catch up
        // incrementally, so it must NOT be forced into a snapshot resume.
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.snapshot_timestamp = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        meta.gc_horizon = Some(Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap());
        // Held horizon is below the snapshot but above the GC mark.
        let held = Utc.with_ymd_and_hms(2026, 5, 25, 0, 0, 0).unwrap();
        assert!(held < meta.snapshot_timestamp && held > meta.gc_horizon_or_min());
        assert!(!snapshot_backstop_trips(held, &meta));
    }

    #[test]
    fn backstop_never_trips_on_a_never_gced_dataset() {
        // No compaction has ever deleted a log → `gc_horizon` is None
        // (→ MIN_UTC), so even a MIN_UTC held horizon must NOT trip. This is
        // what keeps a fresh single-device setup — AND a LEGACY meta that
        // carries a real now()-baseline `snapshot_timestamp` but has no
        // snapshot.json — from wedging in an endless resume loop.
        let fresh = MetaJson::fresh("1.0.0-test");
        assert!(fresh.gc_horizon.is_none());
        assert!(!snapshot_backstop_trips(DateTime::<Utc>::MIN_UTC, &fresh));
        // A legacy-style meta: real snapshot_timestamp, but no GC has happened.
        let mut legacy = MetaJson::fresh("1.0.0-test");
        legacy.snapshot_timestamp = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        legacy.upsert_device(
            &DeviceId::from_string("dev-a".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: false,
            },
        );
        assert!(legacy.has_real_snapshot());
        assert!(
            !snapshot_backstop_trips(DateTime::<Utc>::MIN_UTC, &legacy),
            "a legacy real-baseline meta with no GC must not wedge",
        );
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
