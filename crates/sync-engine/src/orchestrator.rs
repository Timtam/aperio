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

use std::collections::HashSet;
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

/// Cap for [`SYNC_APPLIED_LOG_LENGTHS_PREF_KEY`], the newest files by name.
/// A record must stay while its file can still grow — a live session named
/// long ago included, so no cut by `gc_horizon`: that dropped such a file
/// and its growth was never fetched again (review round 1 of #130). And a
/// file whose record dropped out counts as unknown, which an exact device
/// fetches (DESIGN-sync-snapshot-coverage.md), so the cap sits well above
/// the files 14 days of retention keep (one per device and session).
pub const APPLIED_LOG_LENGTHS_CAP: usize = 256;

/// The log files this device could not read, with their sizes and the
/// parser that tried (JSON, [`UnreadableLogs`]; review rounds 3 to 5 of
/// #130). A file holding a line this build does not know keeps no device from
/// being exact; this device names it in `meta.json`, and no compactor deletes
/// it while it does. Once an update changed the parser, the round reads it
/// again. Device-local.
pub const SYNC_UNREADABLE_LOGS_PREF_KEY: &str = "sync.cursor.unreadableLogs";

/// The record behind [`SYNC_UNREADABLE_LOGS_PREF_KEY`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnreadableLogs {
    /// The parser that could not read them ([`sync_core::PARSER_FINGERPRINT`]);
    /// absent (0) in a record of an earlier build.
    #[serde(default)]
    pub parser: u64,
    /// Filename to size.
    pub files: std::collections::HashMap<String, u64>,
}

impl UnreadableLogs {
    /// The record as `raw` holds it, if this build's parser wrote it: another
    /// one may read what that one could not.
    pub fn of_this_parser(raw: Option<&str>) -> Option<Self> {
        raw.and_then(|raw| serde_json::from_str::<Self>(raw).ok())
            .filter(|record| record.parser == sync_core::PARSER_FINGERPRINT)
    }

    /// The names device `own` publishes in `meta.json`, sorted — whichever
    /// parser recorded them: a file stays named until a round reads it
    /// (review round 6 of #130). Only other devices' files: `own` holds its
    /// own events whatever their remote copy looks like. Its own are in the
    /// record all the same, for the rule that a file torn again at the same
    /// size is no read in progress (review round 7).
    pub fn names(raw: Option<&str>, own: &DeviceId) -> Vec<String> {
        let mut names: Vec<String> = raw
            .and_then(|raw| serde_json::from_str::<Self>(raw).ok())
            .map(|record| record.files.into_keys().collect())
            .unwrap_or_default();
        names.retain(|name| {
            sync_core::LogFileName::from_filename(name).map_or(true, |name| name.device_id != *own)
        });
        names.sort_unstable();
        names
    }
}

/// Set once what a build before 0.2.0 left in `pending/` was settled
/// (`SyncOrchestrator::settle_leftovers`, review round 6 of #130).
/// Device-local.
pub const SYNC_LEFTOVERS_SETTLED_PREF_KEY: &str = "sync.push.leftoversSettled";

/// The leftovers' cutoff, frozen once ([`LeftoversCutoff`], as JSON;
/// `SyncOrchestrator::freeze_leftovers_cutoff`, review rounds 7 to 9 of
/// #130). Device-local.
pub const SYNC_LEFTOVERS_CUTOFF_PREF_KEY: &str = "sync.push.leftoversCutoff";

/// When settling an older build's leftovers last failed to list the remote
/// (`SyncOrchestrator::settle_leftovers`, review round 8 of #130): tried
/// again after [`CATCH_UP_RETRY`]. Device-local.
pub const SYNC_LEFTOVERS_TRIED_AT_PREF_KEY: &str = "sync.push.leftoversTriedAt";

/// The names of the tails settling an older build's leftovers wrote: named
/// before the live file, they are no older build's files, and go up while
/// another leftover waits (review round 10 of #130). Device-local.
pub const SYNC_LEFTOVER_TAILS_PREF_KEY: &str = "sync.push.leftoverTails";

/// The foreign log files an ordinary round read torn, with their sizes
/// (review round 8 of #130): found torn again at the same size, a file is no
/// read in progress and is not fetched every round. Apart from the record a
/// pass over every log trusts ([`SYNC_UNREADABLE_LOGS_PREF_KEY`]): one
/// round's sighting minutes before is no sign that nobody writes the file.
/// Device-local.
pub const SYNC_TORN_SEEN_PREF_KEY: &str = "sync.cursor.tornSeen";

/// What [`SyncOrchestrator::settle_leftovers`] left for the push and the
/// round.
#[derive(Debug, Clone, Copy)]
struct LeftoverHold {
    /// While a leftover is not settled: no finished pending file named
    /// before this goes up.
    below: Option<DateTime<Utc>>,
    /// Whether the leftovers' cutoff stands frozen, or no longer matters. Not
    /// so, the round records no successful round: this build's time would
    /// replace the older build's that the cutoff is read from.
    cutoff_frozen: bool,
}

impl LeftoverHold {
    const SETTLED: Self = Self {
        below: None,
        cutoff_frozen: true,
    };
}

/// What the first try to settle an older build's leftovers froze
/// ([`SYNC_LEFTOVERS_CUTOFF_PREF_KEY`], review rounds 7 to 9 of #130).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeftoversCutoff {
    /// The older build's last successful round (`sync.lastSuccessfulRound`
    /// as it was then), if one is known: what it wrote before went up then.
    pub last_round: Option<DateTime<Utc>>,
    /// The start of the app session that froze it, this build's first: a
    /// pending file named before it is an older build's.
    pub before: DateTime<Utc>,
}

impl LeftoversCutoff {
    /// The frozen cutoff as `raw` holds it; `None` where it does not read.
    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    /// Before when a pending file is an older build's: [`Self::before`] to
    /// the second, as log files are named (review round 10 of #130).
    pub fn older_before(&self) -> DateTime<Utc> {
        self.before.with_nanosecond(0).unwrap_or(self.before)
    }
}

/// When this device last tried its one-time catch-up
/// (DESIGN-sync-snapshot-coverage.md). One that fails or meets a file it
/// cannot read leaves the device not exact; it is tried again after
/// [`CATCH_UP_RETRY`], not every round — it fetches every log file.
/// Device-local, like every `sync.cursor.*` key.
pub const SYNC_CATCH_UP_TRIED_AT_PREF_KEY: &str = "sync.cursor.catchUpTriedAt";

/// How long a device waits before it tries a failed catch-up again.
pub const CATCH_UP_RETRY: chrono::Duration = chrono::Duration::hours(1);

/// What a pending log file sends to the remote: as written — but without
/// credential events while E2E is off (review round 2 of #130). Those exist
/// only from before an E2E downgrade, which purged them from the remote;
/// pushed again from `pending/` after a restart, their secrets would land
/// there in the clear. A line a crash or a full disk left half written goes
/// up as the whole lines around it ([`crate::written_lines`]): as it was, the
/// file never went up while E2E is off and no peer read it while it is on
/// (review round 14). `None` for a file with a line this build cannot read
/// while E2E is off: what it holds cannot be vouched for; and for one with no
/// whole line. The one rule for
/// every way a pending file goes out, and for the compactor weighing whether
/// it has.
pub(crate) fn outgoing_log(log: LogFile, e2e_enabled: bool) -> Option<LogFile> {
    let Some((envelopes, damaged)) = crate::written_lines(&log) else {
        return e2e_enabled.then_some(log);
    };
    // Nothing whole yet — a line being written: nothing goes up.
    if damaged && envelopes.is_empty() {
        return None;
    }
    let is_credential = |envelope: &sync_core::EventEnvelope| {
        matches!(
            envelope.event,
            sync_core::SyncEvent::CredentialSet(_) | sync_core::SyncEvent::CredentialCleared(_)
        )
    };
    let credentials_stay = !e2e_enabled && envelopes.iter().any(is_credential);
    if !damaged && !credentials_stay {
        return Some(log);
    }
    let kept: Vec<_> = envelopes
        .into_iter()
        .filter(|e| e2e_enabled || !is_credential(e))
        .collect();
    LogFile::from_envelopes(log.name.device_id.clone(), log.name.timestamp, &kept).ok()
}

/// RFC 3339 instant from which this device's applied record is EXACT
/// (DESIGN-sync-snapshot-coverage.md): it joined, resumed or caught up over
/// every log file still present, weighing each change against its row, so
/// nothing it records as applied was reverted under the record since. Absent
/// on a device that joined or resumed from a snapshot before this existed —
/// the round catches it up once. Device-local, like every `sync.cursor.*`
/// key.
pub const SYNC_COVERAGE_EXACT_SINCE_PREF_KEY: &str = "sync.cursor.coverageExactSince";

/// Merge freshly applied `(filename, byte length)` records into the
/// serialized map [`SYNC_APPLIED_LOG_LENGTHS_PREF_KEY`] holds, applying
/// the cap (filenames sort chronologically — the timestamp prefix is
/// fixed-width — so a plain string sort ages out the oldest). Returns
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
        self.skipped_older += report.skipped_older;
        self.apply_failures += report.failed;
        self.conflicts += report.conflicts;
        // De-duplicated: several logs in one round can carry the same key
        // (two devices, or one device that changed its mind), and the
        // frontend only needs to know that the key moved.
        for key in report.settings_keys {
            if !self.settings_keys.contains(&key) {
                self.settings_keys.push(key);
            }
        }
        // Sticky across the round's several logs: one log that moved a marker
        // is enough, and a later quiet log must not unset it.
        self.day_markers_touched |= report.day_markers_touched;
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
    async fn resume_from_stale(&self, _adapter: &dyn SyncAdapter) -> SyncResult<ApplyReport> {
        Err(SyncError::internal(
            "auto-resume not supported on this platform",
        ))
    }
    /// Catch this device up once over every log file still present, its own
    /// included (DESIGN-sync-snapshot-coverage.md; Toni, 2026-10-30): its
    /// applied record is not exact yet. A change wins only where it is newer
    /// than its row, without a conflict; afterwards the record is exact. The
    /// default errors: the device keeps its state, as before, and the next
    /// round tries again.
    async fn catch_up(&self, _adapter: &dyn SyncAdapter) -> SyncResult<ApplyReport> {
        Err(SyncError::internal(
            "catch-up not supported on this platform",
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
    /// equality). Deliberately IN-MEMORY: every file still pending is
    /// pushed at least once per app session — safe, because no compactor
    /// deletes a file a device names as pending (review round 6 of #130),
    /// and it repairs a remote copy an interrupted push left torn. A
    /// finished session (named before the writer's live file) is removed
    /// once pushed whole.
    pushed_lengths: Mutex<std::collections::HashMap<String, u64>>,
    /// §19.10: latched stale-device state. Set when a sync
    /// round notices our `meta.devices[me].stale == true`;
    /// cleared by `resume_from_stale` after the snapshot
    /// re-pull. Carries the snapshot timestamp so the resume
    /// dialog can render it.
    stale_device_since: Mutex<Option<DateTime<Utc>>>,
    /// The older build's leftovers whose tail this app session wrote: a later
    /// try only removes the old file (review round 9 of #130).
    tails_written: Mutex<HashSet<String>>,
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

/// One file in `pending/`, read.
struct PendingFile {
    path: PathBuf,
    name: String,
    log: LogFile,
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
            pushed_lengths: Mutex::new(std::collections::HashMap::new()),
            stale_device_since: Mutex::new(None),
            tails_written: Mutex::new(HashSet::new()),
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

    /// Holds rounds and pushes off while something outside a round reads and
    /// writes what they do — the manual stale resume (review round 14 of
    /// #130): beside a round, its later snapshot apply reverted changes the
    /// round had applied, which its earlier listing could not bring back, and
    /// it marked the device exact. Waits for a running round to end, up to
    /// [`HOLD_ROUNDS_WAIT`].
    pub async fn hold_rounds(&self) -> SyncResult<RoundHold<'_>> {
        let deadline = tokio::time::Instant::now() + HOLD_ROUNDS_WAIT;
        loop {
            match InFlightGuard::acquire(&self.in_flight) {
                Ok(guard) => return Ok(RoundHold { _guard: guard }),
                Err(err) if tokio::time::Instant::now() >= deadline => return Err(err),
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
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
        // What a resume or a catch-up below applied, for this round's report.
        let mut recovered: Vec<ApplyReport> = Vec::new();
        // Leftovers of an older build not settled yet: not pushed. Their
        // cutoff is frozen before anything else of the round (review round 9
        // of #130).
        let frozen = self.freeze_leftovers_cutoff();
        let mut hold = self.first_hold(frozen);
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

            // The pending files are named, and older builds locked out, before
            // anything else of the round: a resume or a catch-up takes minutes,
            // and a compactor deleting meanwhile would delete what this device
            // pushes again (review round 7 of #130).
            self.name_pending(adapter.as_ref(), Some(meta), &self.pending_names().await)
                .await;
            // What a build before this one left in pending/, once, before a
            // resume or a catch-up weighs it (review round 6 of #130).
            hold = self.settle_leftovers(adapter.as_ref(), meta, frozen).await;

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
                    Ok(applied) => {
                        info!("§19.10: device was stale; auto-resumed via snapshot re-pull");
                        recovered.push(applied);
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

            self.reread_after_parser_change();

            // DESIGN-sync-snapshot-coverage.md (Toni, 2026-10-30): a device
            // whose record is not exact catches up once over every log file
            // still present — it joined or resumed from a snapshot before
            // this, and a file it skipped by name may hold events it never
            // applied, on any device. The stale resume above already did. A
            // failure leaves the device as it was; the next round retries.
            if !resumed && !self.coverage_exact() && self.catch_up_due() {
                if let Err(err) = self
                    .store
                    .set_pref(SYNC_CATCH_UP_TRIED_AT_PREF_KEY, &Utc::now().to_rfc3339())
                {
                    warn!(?err, "couldn't record the catch-up attempt");
                }
                match self.hooks.catch_up(adapter.as_ref()).await {
                    Ok(applied) => {
                        info!("caught up once over every log file");
                        recovered.push(applied);
                    }
                    Err(err) => warn!(
                        ?err,
                        "catch-up over every log file failed; trying again in an hour",
                    ),
                }
            }
        }
        if resumed {
            round_meta = None;
        }

        let mut report = SyncRoundReport::default();
        // What a resume or a catch-up applied counts as this round's: the
        // platform re-reads settings and reloads views from the report
        // (review round 2 of #130).
        for applied in recovered {
            report.merge_apply(applied);
        }

        // 1. Push pending logs.
        match self
            .push_pending(adapter.as_ref(), round_meta.as_ref(), hold.below)
            .await
        {
            Ok(count) => report.pushed_logs = count,
            Err(err) => {
                warn!(?err, "push phase of sync round failed");
                report.push_failures += 1;
            }
        }

        // 2. Fetch + apply. A device whose record is exact also fetches a
        //    file it has no length for at/below the cursor
        //    (DESIGN-sync-snapshot-coverage.md): a peer's session pushed
        //    late, named before the cursor, or pushed again after a GC,
        //    reached nobody. Only when the record reads as exact — a doubt
        //    keeps the old rule.
        let mut cursor = self.cursor_for_fetch()?;
        cursor.fetch_unknown = matches!(
            self.store.get_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY),
            Ok(Some(_))
        );
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
                //
                // A file holding a line this build cannot read is no torn
                // read: retried every round, it was fetched again and again.
                // Its length is recorded, and the file is named unreadable
                // until an update changed the parser (review round 5 of #130).
                let mut applied_lengths: Vec<(String, u64)> = Vec::new();
                let mut unreadable: Vec<(String, u64)> = Vec::new();
                let mut read: Vec<String> = Vec::new();
                let named: std::collections::HashMap<String, u64> = self
                    .store
                    .get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY)
                    .ok()
                    .flatten()
                    .and_then(|raw| serde_json::from_str::<UnreadableLogs>(&raw).ok())
                    .map(|record| record.files)
                    .unwrap_or_default();
                let torn_seen = self.read_torn_seen();
                let mut torn: Vec<(String, u64)> = Vec::new();
                for log in foreign {
                    let byte_len = log.bytes.len() as u64;
                    let filename = log.name.to_filename();
                    let applied = log
                        .into_envelopes()
                        .map_err(|err| (err, !log.is_torn()))
                        .and_then(|envelopes| {
                            self.applier
                                .apply_envelopes(envelopes)
                                .map_err(|err| (err, false))
                        });
                    match applied {
                        Ok(apply_report) => {
                            report.merge_apply(apply_report);
                            read.push(filename.clone());
                            applied_lengths.push((filename, byte_len));
                        }
                        Err((err, unknown_line)) => {
                            warn!(
                                log = %filename,
                                ?err,
                                unknown_line,
                                "apply phase failed for log file",
                            );
                            report.apply_failures += 1;
                            if unknown_line {
                                applied_lengths.push((filename.clone(), byte_len));
                                unreadable.push((filename, byte_len));
                            } else if named.get(&filename) == Some(&byte_len)
                                || torn_seen.get(&filename) == Some(&byte_len)
                            {
                                // Torn again at the size it is named or was
                                // seen at: no read in progress, so not fetched
                                // every round (review rounds 6 to 8 of #130).
                                applied_lengths.push((filename, byte_len));
                            } else {
                                applied_lengths.push((filename.clone(), 0));
                                torn.push((filename, byte_len));
                            }
                        }
                    }
                }
                // Named first: a file this build cannot read whose name did
                // not keep has no length recorded, so the next round fetches
                // and names it again (review round 9 of #130).
                let unnamed: HashSet<String> =
                    unreadable.iter().map(|(name, _)| name.clone()).collect();
                if !self.update_unreadable(unreadable, &read) {
                    // No length, not 0: a listing without sizes reads 0 as
                    // known (review round 10).
                    applied_lengths.retain(|(name, _)| !unnamed.contains(name));
                }
                self.record_applied_lengths(&applied_lengths);
                self.update_torn_seen(torn, &read);

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
        // Abgleich" even after dozens of successful rounds. Not while the
        // leftovers' cutoff is not frozen: it is read from this pref as an
        // older build left it (review round 8 of #130).
        if !hold.cutoff_frozen {
            warn!("not recording the round: the leftovers' cutoff is not frozen yet");
        } else if let Err(err) = self
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
        self.push_gated(true).await
    }

    /// [`Self::push_now`] for the end-to-end downgrade, the one caller that
    /// has cleared the local E2E flag while this orchestrator's adapter
    /// still encrypts: what is pending goes up encrypted, to be rewritten
    /// as plaintext with the rest of the dataset. Every other gate holds.
    pub async fn flush_before_e2e_downgrade(&self) -> SyncResult<usize> {
        self.push_gated(false).await
    }

    async fn push_gated(&self, encryption_gate: bool) -> SyncResult<usize> {
        let _guard = InFlightGuard::acquire(&self.in_flight)?;
        let adapter = match self.adapter.lock().expect("adapter mutex poison").clone() {
            Some(a) => a,
            None => {
                return Err(sync_core::SyncError::internal("no sync adapter configured"));
            }
        };
        // The round's gates (review round 5 of #130): a plaintext device
        // pushes nothing into an encrypted dataset, a locked-out build
        // nothing at all. And its meta, to name the pending files first.
        let frozen = self.freeze_leftovers_cutoff();
        let meta = adapter.fetch_meta().await?;
        let mut hold = self.first_hold(frozen);
        if let Some(meta) = &meta {
            sync_core::ensure_compatible(meta, &self.hooks.app_version())?;
            if encryption_gate && meta.e2e_enabled && !self.store.e2e_enabled() {
                return Err(sync_core::SyncError::EncryptionRequired);
            }
            self.name_pending(adapter.as_ref(), Some(meta), &self.pending_names().await)
                .await;
            hold = self.settle_leftovers(adapter.as_ref(), meta, frozen).await;
        }
        self.push_pending(adapter.as_ref(), meta.as_ref(), hold.below)
            .await
    }

    /// Push every pending file this app session has not pushed at its
    /// length. Returns the number of successful pushes; per-file errors get
    /// a warning + skip rather than sinking the whole batch.
    ///
    /// The files are named in this device's `meta.json` record first
    /// (`DeviceRecord::pending`, review round 6 of #130): no compactor deletes
    /// a file a device names, so pushing one again — once per app session —
    /// is never pushing a deleted file back, and a remote copy an interrupted
    /// push left torn is whole again. A finished file (named before the one
    /// the writer appends to) is removed once pushed whole. While an older
    /// build's leftovers are not settled, no finished file named before
    /// `hold_below` goes up ([`Self::settle_leftovers`]).
    async fn push_pending(
        &self,
        adapter: &dyn SyncAdapter,
        meta: Option<&sync_core::MetaJson>,
        hold_below: Option<DateTime<Utc>>,
    ) -> SyncResult<usize> {
        let live_at = self.live_at();
        // A tail settling wrote is named before the live file, and is no
        // older build's file (review round 10 of #130).
        let tails = self.leftover_tails();
        let held = |name: &sync_core::LogFileName| {
            hold_below.is_some_and(|below| {
                is_stale_empty_stub(name.timestamp, live_at)
                    && name.timestamp < below
                    && !tails.contains(&name.to_filename())
            })
        };
        let files: Vec<PendingFile> = self
            .read_pending()
            .await?
            .into_iter()
            .filter(|file| !held(&file.log.name))
            .collect();
        let names: Vec<String> = files
            .iter()
            .filter(|file| !file.log.bytes.is_empty())
            .map(|file| file.name.clone())
            .collect();
        self.name_pending(adapter, meta, &names).await;

        let mut pushed = 0usize;
        // Track the newest own-written session file we see so the held
        // horizon (max(cursor, own_newest)) stays current. We record EVERY
        // own session file's timestamp — even an empty current-session stub
        // we skip pushing below — because the file's mere existence means
        // this device holds events up to that session's start.
        let mut newest_own: Option<DateTime<Utc>> = None;
        for file in files {
            let PendingFile { path, name, log } = file;
            if log.name.device_id == self.local_device_id {
                newest_own =
                    Some(newest_own.map_or(log.name.timestamp, |cur| cur.max(log.name.timestamp)));
            }
            // The EventLogWriter pre-creates a session file at app start,
            // before knowing whether the session will produce any events. If
            // it doesn't (e.g. the user opens Aperio, browses, closes), we
            // end up with a 0-byte file in `pending/`. Pushing those would
            // clutter the remote sync folder with empty placeholders — skip
            // + delete the local stub, once the writer can no longer append
            // to it: its session is older than this app launch.
            if log.bytes.is_empty() {
                if is_stale_empty_stub(log.name.timestamp, self.boot_at) {
                    if let Err(err) = tokio::fs::remove_file(&path).await {
                        debug!(name = %name, ?err, "couldn't remove empty pending log");
                    } else {
                        debug!(name = %name, "skipped + removed empty pending log");
                    }
                } else {
                    debug!(name = %name, "skipping empty pending log (current session)");
                }
                continue;
            }
            // Finished: named before the file the writer appends to, so
            // nothing appends to it any more — this device's work, whatever
            // id its name carries (a session under a transient id).
            let finished = is_stale_empty_stub(log.name.timestamp, live_at);
            let byte_count = log.bytes.len() as u64;
            // Skip a file whose byte length matches what this SESSION
            // already pushed — the writer only appends, so equal length
            // means identical content, and re-uploading the whole (all-day
            // growing) session file every round was often the slowest
            // request of the round.
            let already_pushed = self
                .pushed_lengths
                .lock()
                .expect("pushed_lengths mutex poison")
                .get(&name)
                .is_some_and(|len| *len == byte_count);
            if already_pushed {
                debug!(name = %name, "pending log unchanged since last push; skipping");
                continue;
            }
            if self.push_one(adapter, log).await {
                pushed += 1;
                self.pushed_lengths
                    .lock()
                    .expect("pushed_lengths mutex poison")
                    .insert(name.clone(), byte_count);
                // Kept, it went up again after every start (review round 2
                // of #130).
                if finished {
                    match tokio::fs::remove_file(&path).await {
                        Ok(()) => debug!(name = %name, "pushed a finished session; removed it"),
                        Err(err) => {
                            debug!(name = %name, ?err, "couldn't remove a pushed pending log")
                        }
                    }
                }
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

    /// The time the file the writer appends to is named by.
    fn live_at(&self) -> DateTime<Utc> {
        self.compactor
            .writer()
            .map_or(self.boot_at, |writer| writer.session_at())
    }

    /// Name the pending files `names` in this device's `meta.json` record
    /// before any of them goes up, where the record does not name them all
    /// yet: the heartbeat writes them (review round 6 of #130), and locks out
    /// older builds. A failure only warns — the files still go up, and the
    /// next round names them. An empty file is not among them: it never goes
    /// up (review round 7).
    async fn name_pending(
        &self,
        adapter: &dyn SyncAdapter,
        meta: Option<&sync_core::MetaJson>,
        names: &[String],
    ) {
        let Some(meta) = meta else {
            return;
        };
        let named = meta
            .device(&self.local_device_id)
            .map(|record| record.pending.clone())
            .unwrap_or_default();
        if names.iter().all(|name| named.contains(name))
            && meta.schema_version >= sync_core::SCHEMA_VERSION
        {
            return;
        }
        let horizon = match self.held_horizon() {
            Ok(horizon) => horizon,
            Err(err) => {
                warn!(
                    ?err,
                    "couldn't read this device's horizon; pending files not named first"
                );
                return;
            }
        };
        if let Err(err) = self.hooks.heartbeat(adapter, horizon, None).await {
            warn!(
                ?err,
                "couldn't name the pending files in meta.json before pushing them"
            );
        }
    }

    /// What a build before 0.2.0 left in `pending/`, once per device
    /// (review round 6 of #130). Those builds never removed a pushed file,
    /// named nothing as pending, and their compactors deleted by name: a
    /// finished leftover named below `gc_horizon` may be gone from the
    /// remote, and pushing it whole would bring its old events back. Of such
    /// a file the complete lines up to the older build's last successful
    /// round (`frozen`, less ten minutes: a round pushes before it ends, and
    /// an event written during it may have missed its push) went up; the rest
    /// goes into a pending file of its own, named just before the live file,
    /// and the old file goes. With no such round known, nothing counts as
    /// pushed.
    ///
    /// Only where the remote no longer has the file (review round 7): a copy
    /// still there was deleted by no one, and pushed whole again it brings
    /// nothing back — but a later push of an older build may have torn it,
    /// and only the whole file makes it whole again.
    ///
    /// A leftover is a file named before this build first ran and below
    /// `gc_horizon`; until every one is settled, none goes up — one that did
    /// not read, too (review rounds 8 and 9). A try that does not finish is
    /// repeated after [`CATCH_UP_RETRY`]: its listing fetches every file on
    /// most adapters. Done for good once nothing is left.
    async fn settle_leftovers(
        &self,
        adapter: &dyn SyncAdapter,
        meta: &sync_core::MetaJson,
        frozen: Result<Option<LeftoversCutoff>, ()>,
    ) -> LeftoverHold {
        let cutoff = match frozen {
            Ok(None) => return LeftoverHold::SETTLED,
            Ok(Some(cutoff)) => cutoff,
            Err(()) => return self.unfrozen_hold(),
        };
        let mark = |store: &dyn SyncStore| {
            if let Err(err) = store.set_pref(SYNC_LEFTOVERS_SETTLED_PREF_KEY, "1") {
                warn!(?err, "couldn't record that the leftovers are settled");
            }
        };
        let Some(horizon) = meta.gc_horizon else {
            // Nothing was ever deleted.
            mark(self.store.as_ref());
            return LeftoverHold::SETTLED;
        };
        let below = horizon.min(cutoff.older_before());
        let hold = LeftoverHold {
            below: Some(below),
            cutoff_frozen: true,
        };
        let live_at = self.live_at();
        let files = match self.read_pending().await {
            Ok(files) => files,
            Err(err) => {
                warn!(
                    ?err,
                    "couldn't read pending/ to settle what an older build left"
                );
                return hold;
            }
        };
        let is_leftover = |name: &sync_core::LogFileName| {
            is_stale_empty_stub(name.timestamp, live_at) && name.timestamp < below
        };
        let read: HashSet<&str> = files.iter().map(|file| file.name.as_str()).collect();
        let unread = self.pending_names().await.into_iter().any(|name| {
            !read.contains(name.as_str())
                && sync_core::LogFileName::from_filename(&name).is_ok_and(|name| is_leftover(&name))
        });
        if unread {
            warn!("a pending file an older build may have left does not read; settled later");
            return hold;
        }
        let head_cutoff = cutoff
            .last_round
            .map(|at| at - chrono::Duration::minutes(10));
        let leftovers: Vec<(PendingFile, usize)> = files
            .into_iter()
            .filter(|file| !file.log.bytes.is_empty() && is_leftover(&file.log.name))
            .filter_map(|file| {
                let head = head_cutoff.map_or(0, |cutoff| {
                    crate::pushed::head_until(&file.log.bytes, cutoff)
                });
                // Nothing of it known to have gone up: it goes up whole.
                (head > 0).then_some((file, head))
            })
            .collect();
        if !leftovers.is_empty() {
            if !self.leftovers_try_due() {
                return hold;
            }
            let on_remote: HashSet<String> = match adapter.list_log_names().await {
                Ok(names) => names
                    .iter()
                    .map(sync_core::LogFileName::to_filename)
                    .collect(),
                Err(err) => {
                    warn!(
                        ?err,
                        "couldn't list the remote to settle what an older build left"
                    );
                    self.record_leftovers_try();
                    return hold;
                }
            };
            let mut all = true;
            for (PendingFile { path, name, log }, head) in leftovers {
                if on_remote.contains(&name) {
                    continue;
                }
                // A tail this app session wrote already: only the old file
                // is left to go.
                let tail_written = self
                    .tails_written
                    .lock()
                    .expect("tails_written mutex poison")
                    .contains(&name);
                if !tail_written {
                    let tail = log.bytes[head..].to_vec();
                    if !tail.trim_ascii().is_empty()
                        && self
                            .write_tail(&log, tail, live_at, &on_remote)
                            .await
                            .is_none()
                    {
                        all = false;
                        continue;
                    }
                    self.tails_written
                        .lock()
                        .expect("tails_written mutex poison")
                        .insert(name.clone());
                }
                if let Err(err) = tokio::fs::remove_file(&path).await {
                    // Emptied, it is no leftover to a later try, which would
                    // write its tail once more; the push removes it.
                    warn!(name = %name, ?err, "couldn't remove an older build's pushed leftover");
                    if let Err(err) = tokio::fs::write(&path, b"").await {
                        warn!(name = %name, ?err, "couldn't empty it either");
                        all = false;
                    }
                }
            }
            if !all {
                self.record_leftovers_try();
                return hold;
            }
        }
        mark(self.store.as_ref());
        LeftoverHold::SETTLED
    }

    /// Where the leftovers' cutoff is not certain: nothing an earlier app
    /// session left goes up, and the round records no successful round.
    fn unfrozen_hold(&self) -> LeftoverHold {
        LeftoverHold {
            below: Some(self.boot_at.with_nanosecond(0).unwrap_or(self.boot_at)),
            cutoff_frozen: false,
        }
    }

    /// The hold a round or a push starts with, before settling refines it.
    fn first_hold(&self, frozen: Result<Option<LeftoversCutoff>, ()>) -> LeftoverHold {
        match frozen {
            Ok(None) => LeftoverHold::SETTLED,
            Ok(Some(cutoff)) => LeftoverHold {
                below: Some(cutoff.older_before()),
                cutoff_frozen: true,
            },
            Err(()) => self.unfrozen_hold(),
        }
    }

    /// Whether settling the leftovers may try again now
    /// ([`SYNC_LEFTOVERS_TRIED_AT_PREF_KEY`]): no try failed, or the last
    /// failed more than [`CATCH_UP_RETRY`] ago. A mark that does not read
    /// counts as due.
    fn leftovers_try_due(&self) -> bool {
        self.store
            .get_pref(SYNC_LEFTOVERS_TRIED_AT_PREF_KEY)
            .ok()
            .flatten()
            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
            .is_none_or(|at| Utc::now() - at.with_timezone(&Utc) >= CATCH_UP_RETRY)
    }

    fn record_leftovers_try(&self) {
        if let Err(err) = self
            .store
            .set_pref(SYNC_LEFTOVERS_TRIED_AT_PREF_KEY, &Utc::now().to_rfc3339())
        {
            warn!(?err, "couldn't record the try to settle the leftovers");
        }
    }

    /// The leftovers' cutoff ([`LeftoversCutoff`]), frozen by the first call
    /// that finds none: at the start of a round or a push, before anything
    /// of it can move `sync.lastSuccessfulRound` (review rounds 7 to 9 of
    /// #130). `Ok(None)` once the leftovers are settled; `Err` where it is
    /// not certain what was frozen, or the freeze could not be kept. Kept
    /// after the leftovers are settled: a join or a resume reads it.
    fn freeze_leftovers_cutoff(&self) -> Result<Option<LeftoversCutoff>, ()> {
        match self.store.get_pref(SYNC_LEFTOVERS_SETTLED_PREF_KEY) {
            Ok(Some(_)) => return Ok(None),
            Ok(None) => {}
            Err(err) => {
                warn!(?err, "couldn't read whether the leftovers are settled");
                return Err(());
            }
        }
        match self.store.get_pref(SYNC_LEFTOVERS_CUTOFF_PREF_KEY) {
            Ok(Some(raw)) => {
                // One that does not read is frozen anew: kept, it held every
                // join and resume back for good (review round 10 of #130).
                // With no round known — this build's rounds have moved the
                // pref it was read from (review round 11).
                match LeftoversCutoff::parse(&raw) {
                    Some(cutoff) => return Ok(Some(cutoff)),
                    None => {
                        warn!("the leftovers' cutoff does not read; freezing it anew");
                        return self.store_leftovers_cutoff(LeftoversCutoff {
                            last_round: None,
                            before: self.boot_at,
                        });
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                warn!(?err, "couldn't read the leftovers' cutoff");
                return Err(());
            }
        }
        let last_round = match self.store.get_pref(SYNC_LAST_ROUND_PREF_KEY) {
            Ok(raw) => raw
                .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
                .map(|at| at.with_timezone(&Utc)),
            Err(err) => {
                warn!(?err, "couldn't read the last successful round");
                return Err(());
            }
        };
        self.store_leftovers_cutoff(LeftoversCutoff {
            last_round,
            before: self.boot_at,
        })
    }

    fn store_leftovers_cutoff(
        &self,
        cutoff: LeftoversCutoff,
    ) -> Result<Option<LeftoversCutoff>, ()> {
        let raw = serde_json::to_string(&cutoff).map_err(|err| {
            warn!(?err, "couldn't encode the leftovers' cutoff");
        })?;
        if let Err(err) = self.store.set_pref(SYNC_LEFTOVERS_CUTOFF_PREF_KEY, &raw) {
            warn!(?err, "couldn't record the leftovers' cutoff");
            return Err(());
        }
        Ok(Some(cutoff))
    }

    /// The names of the log files in `pending/` that hold anything, without
    /// reading them; none where the directory does not read. What is no file
    /// is none (review round 14 of #130): a directory's size is not 0
    /// everywhere, and named as a leftover it held the settle back for good.
    async fn pending_names(&self) -> Vec<String> {
        let Ok(mut entries) = tokio::fs::read_dir(&self.pending_dir).await else {
            return Vec::new();
        };
        let mut names = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            // An entry whose kind does not read may be a file: it holds back.
            let no_file = entry.file_type().await.is_ok_and(|kind| !kind.is_file());
            let holds_any = !no_file
                && entry
                    .metadata()
                    .await
                    .is_ok_and(|metadata| metadata.len() > 0);
            if holds_any && sync_core::LogFileName::from_filename(&name).is_ok() {
                names.push(name);
            }
        }
        names
    }

    /// Every log file in `pending/`, read.
    async fn read_pending(&self) -> SyncResult<Vec<PendingFile>> {
        let mut entries = match tokio::fs::read_dir(&self.pending_dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Nothing to push — the writer hasn't run yet this session.
                return Ok(Vec::new());
            }
            Err(err) => return Err(sync_core::SyncError::io(err.to_string())),
        };
        let mut files = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| sync_core::SyncError::io(err.to_string()))?
        {
            let path = entry.path();
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let parsed = match sync_core::LogFileName::from_filename(&name) {
                Ok(p) => p,
                Err(_) => {
                    debug!(name = %name, "skipping pending entry: not a log file");
                    continue;
                }
            };
            if entry.file_type().await.is_ok_and(|kind| !kind.is_file()) {
                debug!(name = %name, "skipping pending entry: not a file");
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(err) => {
                    warn!(name = %name, ?err, "couldn't read pending log");
                    continue;
                }
            };
            files.push(PendingFile {
                path,
                name,
                log: LogFile {
                    name: parsed,
                    bytes,
                },
            });
        }
        Ok(files)
    }

    /// Push one file, as it may go out (`outgoing_log`); whether it went.
    async fn push_one(&self, adapter: &dyn SyncAdapter, log: LogFile) -> bool {
        let name = log.name.to_filename();
        let byte_count = log.bytes.len();
        let Some(log) = outgoing_log(log, self.store.e2e_enabled()) else {
            warn!(
                name = %name,
                "pending log does not parse while E2E is off; not pushed this round",
            );
            return false;
        };
        match adapter.push_log(&log).await {
            Ok(()) => {
                // Bump the compactor's "logs since snapshot" counters so its
                // threshold check picks up the new push without an extra
                // round-trip.
                self.compactor.record_pushed_log(byte_count);
                true
            }
            Err(err) => {
                warn!(name = %name, ?err, "push_log failed");
                false
            }
        }
    }

    /// What never went up of an older build's leftover, as a pending file
    /// of its own: named just before the live file, under a name neither a
    /// pending file nor a file on the remote (`taken`) has — a tail an
    /// earlier try pushed is gone from `pending/` (review round 8 of #130).
    /// Named so late, every device fetches it by name too.
    async fn write_tail(
        &self,
        log: &LogFile,
        tail: Vec<u8>,
        live_at: DateTime<Utc>,
        taken: &HashSet<String>,
    ) -> Option<LogFile> {
        let mut at = live_at.with_nanosecond(0).unwrap_or(live_at) - chrono::Duration::seconds(1);
        let name = loop {
            let name = sync_core::LogFileName::new(at, log.name.device_id.clone());
            let filename = name.to_filename();
            if !taken.contains(&filename)
                && !tokio::fs::try_exists(self.pending_dir.join(&filename))
                    .await
                    .unwrap_or(true)
            {
                break name;
            }
            at -= chrono::Duration::seconds(1);
        };
        // Known as a tail before it exists: held as an older build's file
        // otherwise, while another leftover waits.
        if !self.remember_tail(&name.to_filename()) {
            return None;
        }
        let path = self.pending_dir.join(name.to_filename());
        if let Err(err) = tokio::fs::write(&path, &tail).await {
            warn!(name = %name.to_filename(), ?err, "couldn't write what never went up");
            return None;
        }
        Some(LogFile { name, bytes: tail })
    }

    /// The tails settling wrote ([`SYNC_LEFTOVER_TAILS_PREF_KEY`]); none
    /// where they do not read.
    fn leftover_tails(&self) -> HashSet<String> {
        self.store
            .get_pref(SYNC_LEFTOVER_TAILS_PREF_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Records `name` as a tail; whether it was recorded.
    fn remember_tail(&self, name: &str) -> bool {
        let mut tails = self.leftover_tails();
        tails.insert(name.to_string());
        let saved = serde_json::to_string(&tails)
            .map_err(|err| err.to_string())
            .and_then(|raw| {
                self.store
                    .set_pref(SYNC_LEFTOVER_TAILS_PREF_KEY, &raw)
                    .map_err(|err| err.to_string())
            });
        if let Err(err) = saved {
            warn!(%err, "couldn't record the tail of an older build's leftover");
            return false;
        }
        true
    }

    /// Whether the catch-up is due ([`SYNC_CATCH_UP_TRIED_AT_PREF_KEY`]):
    /// never tried, or tried more than [`CATCH_UP_RETRY`] ago. A mark that
    /// does not read counts as due.
    fn catch_up_due(&self) -> bool {
        self.store
            .get_pref(SYNC_CATCH_UP_TRIED_AT_PREF_KEY)
            .ok()
            .flatten()
            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
            .is_none_or(|at| Utc::now() - at.with_timezone(&Utc) >= CATCH_UP_RETRY)
    }

    /// Whether this device's applied record is exact
    /// ([`SYNC_COVERAGE_EXACT_SINCE_PREF_KEY`]). An unreadable record counts
    /// as exact: guessing "not exact" would catch the device up every round
    /// for as long as the read fails.
    fn coverage_exact(&self) -> bool {
        match self.store.get_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY) {
            Ok(mark) => mark.is_some(),
            Err(err) => {
                warn!(?err, "couldn't read whether this device's record is exact");
                true
            }
        }
    }

    /// A file this build's parser could not read is read again once an
    /// update changed the parser (review rounds 5 and 6 of #130): its
    /// recorded length goes, so the next fetch takes it as unknown — also
    /// from an adapter that lists no sizes. It stays named until a
    /// round reads it — a fetch failing meanwhile must not lift the GC's
    /// guard on a file this device never applied. A record that does not
    /// read names nothing certain: the device catches up once over every
    /// file.
    fn reread_after_parser_change(&self) {
        let Ok(Some(raw)) = self.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY) else {
            return;
        };
        match serde_json::from_str::<UnreadableLogs>(&raw) {
            Ok(record) if record.parser == sync_core::PARSER_FINGERPRINT => {}
            // Under the current parser only once the lengths are forgotten:
            // the next round tries again (review round 7 of #130).
            Ok(record) => {
                if self.forget_applied_lengths(record.files.keys()) {
                    self.save_unreadable(&UnreadableLogs {
                        parser: sync_core::PARSER_FINGERPRINT,
                        files: record.files,
                    });
                }
            }
            Err(err) => {
                warn!(
                    ?err,
                    "the record of unreadable log files does not read; catching up once"
                );
                if let Err(err) = self.store.delete_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY) {
                    warn!(?err, "couldn't clear the exact mark");
                }
                if let Err(err) = self.store.delete_pref(SYNC_UNREADABLE_LOGS_PREF_KEY) {
                    warn!(?err, "couldn't clear the record of unreadable log files");
                }
            }
        }
    }

    /// The record of unreadable log files with `added` named and `read` —
    /// files this round applied — no longer. Whatever parser stamped it: one
    /// a parser change did not stamp anew yet still names what the new parser
    /// is to read (review round 8 of #130). A record that does not read stays
    /// as it is.
    /// Whether the record holds `added` now — not so where it does not read
    /// or does not save (review round 9 of #130).
    fn update_unreadable(&self, added: Vec<(String, u64)>, read: &[String]) -> bool {
        let raw = match self.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY) {
            Ok(raw) => raw,
            Err(err) => {
                warn!(?err, "couldn't read the record of unreadable log files");
                return added.is_empty();
            }
        };
        let before = raw
            .as_deref()
            .and_then(|raw| serde_json::from_str::<UnreadableLogs>(raw).ok());
        if before.is_none() && added.is_empty() {
            return true;
        }
        let mut record = before.clone().unwrap_or(UnreadableLogs {
            parser: sync_core::PARSER_FINGERPRINT,
            files: Default::default(),
        });
        for name in read {
            record.files.remove(name);
        }
        record.files.extend(added);
        Some(&record) == before.as_ref() || self.save_unreadable(&record)
    }

    /// The torn sightings of ordinary rounds ([`SYNC_TORN_SEEN_PREF_KEY`]);
    /// none where they do not read.
    fn read_torn_seen(&self) -> std::collections::HashMap<String, u64> {
        self.store
            .get_pref(SYNC_TORN_SEEN_PREF_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// The torn sightings with `torn` added and `read` — files this round
    /// applied — dropped.
    fn update_torn_seen(&self, torn: Vec<(String, u64)>, read: &[String]) {
        let before = self.read_torn_seen();
        let mut seen = before.clone();
        for name in read {
            seen.remove(name);
        }
        seen.extend(torn);
        if seen == before {
            return;
        }
        let saved = if seen.is_empty() {
            self.store.delete_pref(SYNC_TORN_SEEN_PREF_KEY)
        } else {
            match serde_json::to_string(&seen) {
                Ok(raw) => self.store.set_pref(SYNC_TORN_SEEN_PREF_KEY, &raw),
                Err(err) => {
                    warn!(?err, "couldn't encode the torn sightings");
                    return;
                }
            }
        };
        if let Err(err) = saved {
            warn!(?err, "couldn't record the torn sightings");
        }
    }

    /// Whether `record` was saved.
    fn save_unreadable(&self, record: &UnreadableLogs) -> bool {
        let saved = if record.files.is_empty() {
            self.store.delete_pref(SYNC_UNREADABLE_LOGS_PREF_KEY)
        } else {
            match serde_json::to_string(record) {
                Ok(raw) => self.store.set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, &raw),
                Err(err) => {
                    warn!(?err, "couldn't encode the unreadable log files");
                    return false;
                }
            }
        };
        if let Err(err) = saved {
            warn!(?err, "couldn't record the unreadable log files");
            return false;
        }
        true
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
                    fetch_unknown: false,
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
    /// Forget the recorded lengths of `names`: the next fetch takes them as
    /// files it does not know. Whether none of them is recorded any more.
    fn forget_applied_lengths<'a>(&self, names: impl Iterator<Item = &'a String>) -> bool {
        let raw = match self.store.get_pref(SYNC_APPLIED_LOG_LENGTHS_PREF_KEY) {
            Ok(Some(raw)) => raw,
            Ok(None) => return true,
            Err(err) => {
                warn!(?err, "couldn't read the applied-log-length map");
                return false;
            }
        };
        // A map that does not read records no length at all.
        let Ok(mut map) = serde_json::from_str::<std::collections::HashMap<String, u64>>(&raw)
        else {
            return true;
        };
        for name in names {
            map.remove(name);
        }
        match serde_json::to_string(&map) {
            Ok(raw) => match self.store.set_pref(SYNC_APPLIED_LOG_LENGTHS_PREF_KEY, &raw) {
                Ok(()) => true,
                Err(err) => {
                    warn!(?err, "couldn't persist applied-log-length map");
                    false
                }
            },
            Err(err) => {
                warn!(?err, "couldn't encode applied-log-length map");
                false
            }
        }
    }

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

/// How long [`SyncOrchestrator::hold_rounds`] waits for a running round: a
/// round over a slow WebDAV share can take minutes.
pub const HOLD_ROUNDS_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// No round or push runs while this is held ([`SyncOrchestrator::hold_rounds`]).
pub struct RoundHold<'a> {
    _guard: InFlightGuard<'a>,
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
    #[test]
    fn the_outgoing_form_of_a_pending_log_drops_secrets_only_while_e2e_is_off() {
        let device = DeviceId::from_string("dev-a".into());
        let at = Utc::now();
        let secret = sync_core::EventEnvelope::new(
            device.clone(),
            sync_core::SyncEvent::CredentialCleared(sync_core::CredentialSlotPayload {
                account_id: "acc-1".into(),
                slot: "password".into(),
            }),
        );
        let setting = sync_core::EventEnvelope::new(
            device.clone(),
            sync_core::SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
                key: "appearance.darkMode".into(),
                value: serde_json::json!(true),
            }),
        );
        let log = LogFile::from_envelopes(device.clone(), at, &[secret, setting.clone()]).unwrap();

        assert_eq!(outgoing_log(log.clone(), true), Some(log.clone()));
        let out = outgoing_log(log, false).unwrap();
        assert_eq!(out.into_envelopes().unwrap(), vec![setting.clone()]);
        let plain = LogFile::from_envelopes(device.clone(), at, &[setting]).unwrap();
        assert_eq!(outgoing_log(plain.clone(), false), Some(plain), "unchanged");
        let torn = LogFile {
            name: sync_core::LogFileName::new(at, device),
            bytes: b"{\"id\":".to_vec(),
        };
        assert_eq!(outgoing_log(torn, false), None);
    }

    #[test]
    fn a_pending_log_a_crash_damaged_goes_up_as_its_whole_lines() {
        // Review round 14 of #130: it never went up while E2E was off, and
        // no peer read it while it was on.
        let device = DeviceId::from_string("dev-a".into());
        let at = Utc::now();
        let setting = |key: &str| {
            sync_core::EventEnvelope::new(
                device.clone(),
                sync_core::SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
                    key: key.into(),
                    value: serde_json::json!(true),
                }),
            )
        };
        let secret = sync_core::EventEnvelope::new(
            device.clone(),
            sync_core::SyncEvent::CredentialCleared(sync_core::CredentialSlotPayload {
                account_id: "acc-1".into(),
                slot: "password".into(),
            }),
        );
        let (a, b) = (setting("a"), setting("b"));
        let mut bytes = Vec::new();
        for (envelope, after) in [(&a, "\n{\"id\":\"half\n"), (&secret, "\n"), (&b, "\n\0\0")] {
            bytes.extend(serde_json::to_vec(envelope).unwrap());
            bytes.extend(after.as_bytes());
        }
        let log = LogFile {
            name: sync_core::LogFileName::new(at, device.clone()),
            bytes,
        };

        let on = outgoing_log(log.clone(), true).unwrap();
        assert_eq!(
            on.into_envelopes().unwrap(),
            vec![a.clone(), secret, b.clone()]
        );
        let off = outgoing_log(log, false).unwrap();
        assert_eq!(off.into_envelopes().unwrap(), vec![a, b]);
    }

    use super::{
        is_stale_empty_stub, merge_applied_log_lengths, outgoing_log, snapshot_backstop_trips,
    };
    use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
    use sync_core::LogFile;
    use sync_core::{DeviceId, DeviceRecord, MetaJson};

    #[test]
    fn applied_lengths_keep_the_newest_files_whatever_their_age() {
        // A record must stay while its file can still grow — a live session
        // named long ago included. Cutting records by the GC horizon dropped
        // such a file, and its growth was never fetched again (review round 1
        // of #130). Only the count cap ages records out, oldest name first.
        let at = |secs: i64| {
            sync_core::LogFileName::new(
                Utc.timestamp_opt(secs, 0).unwrap(),
                DeviceId::from_string("peer".into()),
            )
            .to_filename()
        };
        let live = at(500);
        let mut fetched: Vec<(String, u64)> = (0..100).map(|n| (at(10_000 + n), 7)).collect();
        fetched.push((live.clone(), 3));
        let raw = merge_applied_log_lengths(None, &fetched).unwrap();
        let map: std::collections::HashMap<String, u64> = serde_json::from_str(&raw).unwrap();
        assert_eq!(map.len(), 101);
        assert_eq!(
            map.get(&live),
            Some(&3),
            "an old live session keeps its record"
        );

        let many: Vec<(String, u64)> = (0..300).map(|n| (at(20_000 + n), 1)).collect();
        let raw = merge_applied_log_lengths(Some(&raw), &many).unwrap();
        let map: std::collections::HashMap<String, u64> = serde_json::from_str(&raw).unwrap();
        assert_eq!(map.len(), super::APPLIED_LOG_LENGTHS_CAP);
        assert!(
            !map.contains_key(&live),
            "beyond the cap, the oldest name goes"
        );
        assert!(map.contains_key(&at(20_299)));
    }

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
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
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
