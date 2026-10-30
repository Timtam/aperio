//! Log compaction (DESIGN.md §19.10, Phase Sg).
//!
//! Long-running datasets accumulate one JSONL file per app session
//! per device. Without compaction a year of daily use would leave
//! hundreds of files for a new onboarding device to download and
//! replay. The compactor folds everything older than a chosen
//! cutoff into a single `snapshot.json`, then GCs the now-redundant
//! log files.
//!
//! ## Algorithm (from §19.10)
//!
//! ```text
//! 1. Build a snapshot of the current local state and push it to
//!    the remote (replacing any prior snapshot.json). It is stamped
//!    snapshot_ts = max(own_newest_log, fetch_cursor) — this device's
//!    real held content, NOT now() — so a joining device adopts a
//!    cursor that matches what the snapshot actually covers, and the
//!    snapshot never claims to cover an unapplied foreign log.
//! 2. Update meta.json.snapshot_timestamp to snapshot_ts; stamp this
//!    device's own record at the same point.
//! 3. Compute safe_cutoff = max(min held horizon across devices,
//!    snapshot_ts - retention). The first term never GCs a log a
//!    still-tracked device hasn't covered (so a briefly-behind peer or
//!    a concurrent lower-horizon compactor can still catch up — the
//!    data-loss guard); the second caps how far one chronically-offline
//!    device holds the cutoff back (the pile-up bound). Publish
//!    gc_horizon = max(prior gc_horizon, safe_cutoff): MONOTONIC, since
//!    deletions are permanent.
//! 4. Mark devices whose held horizon (last_seen_log) < gc_horizon as
//!    stale — the logs they'd replay are GONE, so they consume the
//!    snapshot on return (the §19.10 gate). A device behind the SNAPSHOT
//!    but at/above gc_horizon is left alone: the retained logs let it
//!    catch up incrementally (the over-fire fix keys staleness on what
//!    was deleted, not on snapshot content).
//!    The snapshot names the events still in log files that it holds
//!    (own or applied), when this device's record is exact and it holds
//!    every readable one (DESIGN-sync-snapshot-coverage.md).
//! 5. Delete every log with timestamp STRICTLY < safe_cutoff whose every
//!    event, as listed afresh after the snapshot, the snapshot holds; never
//!    an empty file or one that does not parse. Strict `<` spares the
//!    freshest covered log one round.
//! 6. Reset the local "bytes/logs since snapshot" counters so the
//!    threshold check doesn't immediately re-fire.
//! ```
//!
//! ## Concurrency
//!
//! Multiple devices could compact simultaneously. Last-write-wins
//! on `snapshot.json` and `meta.json` is tolerated by the §19.5
//! design — at worst one of the runs is redundant work. The
//! local-FS adapter offers no etag locking; WebDAV (Phase Sj) will
//! upgrade this to optimistic concurrency.
//!
//! ## Trigger thresholds (§19.10)
//!
//! Three knobs, any of which independently dispatches compaction:
//!
//! | Pref key                          | Default | Meaning                                 |
//! |-----------------------------------|---------|-----------------------------------------|
//! | `sync.compaction.maxAgeDays`      | 30      | Days since last snapshot.               |
//! | `sync.compaction.maxLogs`         | 1000    | Logs this device pushed since snapshot. |
//! | `sync.compaction.maxBytes`        | 50 MB   | Bytes this device pushed since snapshot.|
//!
//! "Logs this device pushed" is a per-device proxy for the
//! design-spec "log events". Across a multi-device dataset the
//! actual event count is higher; the threshold still triggers
//! reasonably often on its own without needing to walk every
//! remote log to count events exactly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use serde::Serialize;
use sync_core::{
    DeviceCursor, DeviceId, DeviceRecord, LogFile, LogFileName, MetaJson, SyncAdapter, SyncResult,
};
use tracing::{debug, info, warn};

use crate::{
    AperioSnapshotBody, EventLogWriter, SnapshotApplyOutcome, SnapshotBuilder, SyncStore,
    PREF_DEVICE_NAME, SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, SYNC_CURSOR_PREF_KEY,
    SYNC_OWN_NEWEST_LOG_PREF_KEY,
};

/// `user_prefs` keys the compactor reads + writes.
pub const PREF_MAX_AGE_DAYS: &str = "sync.compaction.maxAgeDays";
pub const PREF_MAX_LOGS: &str = "sync.compaction.maxLogs";
pub const PREF_MAX_BYTES: &str = "sync.compaction.maxBytes";
/// Local counter: number of logs this device pushed since the
/// last successful compaction. Incremented by the orchestrator on
/// every `push_log` success; cleared by the compactor when it
/// finishes a round.
pub const PREF_LOGS_SINCE_SNAPSHOT: &str = "sync.compaction.localLogsSinceSnapshot";
/// Local counter for bytes pushed, same lifecycle as
/// [`PREF_LOGS_SINCE_SNAPSHOT`].
pub const PREF_BYTES_SINCE_SNAPSHOT: &str = "sync.compaction.localBytesSinceSnapshot";

/// How many days of pre-snapshot logs to RETAIN behind a device that has
/// fallen behind, before abandoning it to a snapshot resume. This is the cap
/// on how far a single behind/offline device can hold the GC cutoff back: logs
/// newer than `snapshot_ts - retention` are kept so a briefly-behind device
/// (or a concurrent compactor at a lower horizon) can still catch up across
/// them incrementally; logs older than that are GC'd even if a chronically
/// offline device still wanted them (it consumes the snapshot on return). This
/// is the knob that bounds the "old logs pile up behind one offline device"
/// growth without the concurrent-compaction data-loss the unbounded-aggressive
/// GC caused.
pub const PREF_GC_RETENTION_DAYS: &str = "sync.compaction.logRetentionDays";

pub const DEFAULT_MAX_AGE_DAYS: u32 = 30;
pub const DEFAULT_MAX_LOGS: u32 = 1000;
pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024 * 1024;
/// Default for [`PREF_GC_RETENTION_DAYS`]. Two weeks: long enough that a device
/// offline over a normal trip still catches up incrementally, short enough that
/// the sync folder doesn't accumulate unbounded log files behind a device that
/// has effectively gone away.
pub const DEFAULT_GC_RETENTION_DAYS: u32 = 14;

/// Outcome counters surfaced to the frontend via `compact_now`.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CompactionReport {
    pub snapshot_timestamp: Option<String>,
    pub deleted_logs: usize,
    pub failed_deletes: usize,
    pub stale_devices: usize,
    /// Number of rows + settings the snapshot generation operated
    /// on. Surfaced so the user sees "snapshotted 2400 rows" in
    /// the Settings dialog.
    pub snapshot_rows: usize,
    pub snapshot_settings: usize,
    /// Whether the snapshot names the events it holds — false when this
    /// device could not vouch for its applied record: not exact yet, or an
    /// event in a file it neither wrote nor applied.
    pub snapshot_coverage: bool,
}

/// What the log files on the remote hold that this device's state reflects
/// (DESIGN-sync-snapshot-coverage.md). An event is HELD when it is this
/// device's own (its database has every own change) or recorded as applied.
struct Coverage {
    /// Every held event's id, in file order.
    held: Vec<String>,
    /// False when a readable foreign event is neither own nor applied: this
    /// device cannot vouch for its record then, and the snapshot names
    /// nothing. A file that does not parse is no gap: no device of this
    /// version can apply it.
    complete: bool,
}

impl Coverage {
    fn of(
        logs: &[LogFile],
        local_device: &DeviceId,
        store: &dyn SyncStore,
    ) -> SyncResult<Coverage> {
        let mut coverage = Coverage {
            held: Vec::new(),
            complete: true,
        };
        for log in logs {
            let Ok(envelopes) = log.into_envelopes() else {
                continue;
            };
            for envelope in envelopes {
                if is_held(&envelope, local_device, store)? {
                    coverage.held.push(envelope.id);
                } else {
                    coverage.complete = false;
                }
            }
        }
        Ok(coverage)
    }

    /// Whether the GC may delete `log`: every event in it is held, as it
    /// reads now. Never an empty file or one that does not parse — it may be
    /// a torn read of a file being written, or hold what a newer version
    /// wrote.
    fn deletable(
        log: &LogFile,
        local_device: &DeviceId,
        store: &dyn SyncStore,
    ) -> SyncResult<bool> {
        let Ok(envelopes) = log.into_envelopes() else {
            return Ok(false);
        };
        if envelopes.is_empty() {
            return Ok(false);
        }
        for envelope in &envelopes {
            if !is_held(envelope, local_device, store)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// An event is held when it is this device's own (its database has every
/// own change) or recorded as applied.
fn is_held(
    envelope: &sync_core::EventEnvelope,
    local_device: &DeviceId,
    store: &dyn SyncStore,
) -> SyncResult<bool> {
    if envelope.device_id == *local_device {
        return Ok(true);
    }
    store
        .is_event_applied(&envelope.id)
        .map_err(|err| sync_core::SyncError::internal(format!("read applied events: {err}")))
}

impl CompactionReport {
    fn record_snapshot(&mut self, ts: DateTime<Utc>, outcome: SnapshotApplyOutcome) {
        self.snapshot_timestamp = Some(ts.to_rfc3339());
        // The applier's `rows_applied` is the dump size at build
        // time — surface that as the snapshot row count.
        self.snapshot_rows = outcome.rows_applied;
        self.snapshot_settings = outcome.settings_applied;
    }
}

/// The compactor. Holds the same handles as the orchestrator so it
/// can run a snapshot build + apply round-trip without any other
/// services.
pub struct Compactor {
    /// Local store seam — the engine's device-local prefs: the
    /// compaction counters + thresholds and the device name.
    store: Arc<dyn SyncStore>,
    builder: Arc<SnapshotBuilder>,
    local_device_id: DeviceId,
    app_version: String,
    /// Writer handle, used to roll the local session file over before
    /// snapshotting (see `compact_now`). `None` in tests / headless
    /// paths without a writer — compaction then runs as before, just
    /// without the redundant-push optimisation.
    writer: Option<Arc<EventLogWriter>>,
    /// `<data_dir>/sync/log/pending/` — the local staging dir the
    /// orchestrator pushes from. After a snapshot is pushed, every file
    /// here older than the cut is redundant, so the compactor sweeps them
    /// out; otherwise old leftovers (e.g. from a prior crash) get
    /// re-uploaded on every push and never disappear. `None` in tests /
    /// headless paths that don't stage to disk.
    pending_dir: Option<PathBuf>,
}

impl Compactor {
    pub fn new(
        store: Arc<dyn SyncStore>,
        builder: Arc<SnapshotBuilder>,
        local_device_id: DeviceId,
        app_version: impl Into<String>,
        writer: Option<Arc<EventLogWriter>>,
        pending_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            store,
            builder,
            local_device_id,
            app_version: app_version.into(),
            writer,
            pending_dir,
        }
    }

    /// Should we run compaction right now? Reads each threshold
    /// from `user_prefs` (with defaults) and compares against
    /// either `meta.snapshot_timestamp` (age check) or the local
    /// since-snapshot counters (log count + byte size).
    ///
    /// `round_meta` is the meta the sync round already fetched — reused
    /// so the threshold check costs no extra GET. A caller without one
    /// (the manual compact path) passes `None` and we fetch. The age
    /// anchor being seconds old is irrelevant at day-granularity.
    pub async fn should_compact(
        &self,
        adapter: &dyn SyncAdapter,
        round_meta: Option<&sync_core::MetaJson>,
    ) -> SyncResult<bool> {
        // Age threshold uses meta.snapshot_timestamp as the
        // anchor. If meta is missing, treat the dataset as
        // brand-new — no compaction needed yet.
        let meta = match round_meta {
            Some(m) => Some(m.clone()),
            None => adapter.fetch_meta().await?,
        };
        if let Some(meta) = &meta {
            // Only an existing REAL snapshot has a meaningful age. A
            // never-compacted dataset carries the `MIN_UTC` sentinel, whose
            // "age" would be enormous — gating on `has_real_snapshot`
            // stops that from force-triggering compaction on a fresh
            // dataset (the log-count / byte thresholds drive the first
            // compaction instead).
            if meta.has_real_snapshot() {
                let max_age = self.read_pref_u32(PREF_MAX_AGE_DAYS, DEFAULT_MAX_AGE_DAYS);
                let age = Utc::now() - meta.snapshot_timestamp;
                if age > ChronoDuration::days(i64::from(max_age)) {
                    debug!(
                        days = age.num_days(),
                        threshold_days = max_age,
                        "compaction triggered: age threshold breached",
                    );
                    return Ok(true);
                }
            }
        }

        let max_logs = self.read_pref_u32(PREF_MAX_LOGS, DEFAULT_MAX_LOGS);
        let logs_since = self.read_counter_u64(PREF_LOGS_SINCE_SNAPSHOT);
        if logs_since >= u64::from(max_logs) {
            debug!(
                count = logs_since,
                threshold = max_logs,
                "compaction triggered: log-count threshold breached",
            );
            return Ok(true);
        }

        let max_bytes = self.read_pref_u64(PREF_MAX_BYTES, DEFAULT_MAX_BYTES);
        let bytes_since = self.read_counter_u64(PREF_BYTES_SINCE_SNAPSHOT);
        if bytes_since >= max_bytes {
            debug!(
                bytes = bytes_since,
                threshold = max_bytes,
                "compaction triggered: byte-size threshold breached",
            );
            return Ok(true);
        }

        Ok(false)
    }

    /// Run one round of compaction. See module docs for the
    /// algorithm. Returns a [`CompactionReport`] with the
    /// observable side-effects.
    pub async fn compact_now(&self, adapter: &dyn SyncAdapter) -> SyncResult<CompactionReport> {
        // Roll the writer over to a fresh session file BEFORE building the
        // snapshot. Two payoffs:
        //   - the snapshot (built from SQLite right after the old file
        //     closes) captures every event that file holds, so the file is
        //     redundant and we delete it locally instead of re-uploading
        //     already-snapshotted events on the next push;
        //   - post-compaction edits land in the NEW file, stamped `cut`,
        //     which is one second NEWER than the snapshot (`cut - 1s`). A
        //     device that consumes the snapshot advances its cursor to the
        //     snapshot timestamp and only fetches newer logs — so it still
        //     picks up the new file instead of skipping it as
        //     "older than the snapshot".
        // `snapshot_ts` is bounded to this device's ACTUAL content horizon —
        // `max(newest own-written log, foreign fetch cursor)` — not a bare
        // `now() - 1s`. Two distinct bugs motivated this:
        //   - Over-fire: `now()` put the horizon a hair above every real log
        //     file, so a fully caught-up device's held horizon could never
        //     reach it and the §19.10 backstop flagged healthy devices stale
        //     on normal rounds. Anchoring on real content lets a caught-up
        //     `max(cursor, own)` land exactly on the horizon and pass.
        //   - Data loss: the cursor only covers APPLIED foreign logs, so the
        //     newest remote log could include a foreign log we haven't
        //     applied. Bounding by our own content stops the snapshot from
        //     advertising coverage of events it doesn't actually hold.
        //   - Unreadable: both reads propagate rather than collapsing to the
        //     `MIN_UTC` floor, which would look like "no content yet" and take
        //     the brand-new-dataset branch below.
        let own_newest = self.read_own_newest_log()?;
        let cursor = self.read_cursor()?;
        let content_horizon = own_newest.max(cursor);

        // Refuse to compact when THIS device is itself behind the published GC
        // horizon. The auto path gates on the §19.10 backstop before reaching
        // here, but the MANUAL "compact now" command (desktop + mobile) calls
        // straight in. A device whose content horizon is below `gc_horizon`
        // would push a snapshot covering LESS than the dataset claims to have
        // already GC'd — opening a `(content_horizon, gc_horizon]` window that a
        // peer running a snapshot resume would silently skip. Send it through
        // the resume flow instead (StaleDevice → the §19.10 dialog), which is
        // exactly what a normal round would do; this also skips the wasted
        // snapshot build. (A concurrent compactor can still raise `gc_horizon`
        // after this check, but the conservative `min_device_held` floor means
        // it could not have deleted any log in our window, so the worst case is
        // one redundant resume, not data loss.)
        if let Some(existing) = adapter.fetch_meta().await? {
            if content_horizon < existing.gc_horizon_or_min() {
                return Err(sync_core::SyncError::StaleDevice {
                    snapshot_at: existing.snapshot_timestamp.to_rfc3339(),
                });
            }
        }
        // Whole-second because log filenames are second-granular.
        let now = Utc::now().with_nanosecond(0).unwrap_or_else(Utc::now);
        // Fall back to `now - 1s` only when there's no content yet (a brand
        // new dataset compacted before any log was written): the snapshot
        // still needs a real, monotonic timestamp distinct from the MIN_UTC
        // "no snapshot" sentinel.
        let snapshot_ts = if content_horizon > DateTime::<Utc>::MIN_UTC {
            content_horizon
        } else {
            now - ChronoDuration::seconds(1)
        };
        // The post-rotation session file must sort STRICTLY AFTER the
        // snapshot, so a device that consumes the snapshot (cursor →
        // snapshot_ts) still fetches post-compaction edits instead of
        // skipping them as "older than the snapshot". Guarantee that even
        // when the content horizon is close to `now`.
        let cut = now.max(snapshot_ts + ChronoDuration::seconds(1));
        // Roll the live session file over to a fresh one stamped `cut`, so
        // the snapshot below captures everything the old file holds and
        // post-compaction edits land in a file NEWER than the snapshot.
        if let Some(writer) = &self.writer {
            let _ = writer.rotate(cut).await;
        }

        // 1. Build + push the snapshot, stamped just before the new file.
        //    It names the events it holds that are still in log files, so a
        //    device taking it can fetch every file and apply only the rest —
        //    but only when this device can vouch for its record: it is exact
        //    (it joined or caught up over every file, so nothing it applied
        //    was reverted under the record), and no file holds an event it
        //    neither wrote nor applied (a peer's file grew since its last
        //    fetch). Otherwise it names nothing.
        let all_logs = adapter.fetch_new_logs(&DeviceCursor::epoch()).await?;
        let coverage = Coverage::of(&all_logs, &self.local_device_id, self.store.as_ref())?;
        let names = coverage.complete && self.record_exact();
        let snapshot = self
            .builder
            .build_at(snapshot_ts, names.then(|| coverage.held.clone()))?;
        adapter.push_snapshot(&snapshot).await?;

        // The snapshot is now durable on the remote, so every LOCAL pending
        // file stamped before `cut` is folded into it and must not be
        // re-uploaded on every push. Sweep ALL of them — not just the file
        // we just rotated away: old leftovers (e.g. from a prior crash that
        // left logs in `pending/`) would otherwise be re-pushed on every
        // round and never disappear. The live post-rotation file is stamped
        // `cut` itself, so the strict `< cut` check keeps it. But peers take
        // log files, not the snapshot: what a file gained since its last
        // push is uploaded first (review round 1 of #130), and a file that
        // fails to upload stays for the next round's push.
        if let Some(pending) = &self.pending_dir {
            let remote_lengths: HashMap<String, usize> = all_logs
                .iter()
                .map(|log| (log.name.to_filename(), log.bytes.len()))
                .collect();
            sweep_redundant_pending(pending, cut, adapter, &remote_lengths).await;
        }

        let mut report = CompactionReport::default();
        // Surface the snapshot row counters via a fake "apply" of
        // our own snapshot — same accounting the consumer side
        // will report. Done locally so we don't actually
        // re-write our own DB rows.
        let outcome = self.snapshot_size_counters(&snapshot);
        report.record_snapshot(snapshot_ts, outcome);

        // 2. Load + update meta.
        let mut meta = adapter
            .fetch_meta()
            .await?
            .unwrap_or_else(|| MetaJson::fresh(&self.app_version));
        // §19.13: if this app writes a newer sync wire format than the dataset
        // currently records, the snapshot we just generated IS that migrated
        // artifact — bump schema_version and raise min_app_version so apps too
        // old to read the new format stop syncing (the "force snapshot + meta
        // update on schema upgrade" step). A no-op while SCHEMA_VERSION is
        // unchanged, which is the common case.
        if meta.schema_version < sync_core::SCHEMA_VERSION {
            meta.schema_version = sync_core::SCHEMA_VERSION;
            meta.min_app_version = self.app_version.clone();
        }
        meta.snapshot_timestamp = snapshot_ts;
        let device_name = self.store.get_pref(PREF_DEVICE_NAME).ok().flatten();
        meta.upsert_device(
            &self.local_device_id,
            DeviceRecord {
                name: device_name,
                last_seen_log: snapshot_ts,
                // `upsert_device` replaces the whole record, so leaving this
                // `None` would ERASE the stamp the heartbeat wrote — and a
                // compaction is itself the strongest evidence this device was
                // here, so the one moment it is most certainly true is the
                // moment the answer would have gone missing.
                last_seen: Some(Utc::now()),
                app_version: self.app_version.clone(),
                stale: false,
            },
        );

        // 3. Compute the GC cutoff = max(lowest held horizon across all
        //    registered devices, snapshot_ts - retention). The two terms split
        //    the responsibility:
        //      - `min_device_held` is the conservative floor: never GC a log a
        //        still-tracked device hasn't covered, so a briefly-behind
        //        device — OR a concurrent compactor sitting at a lower horizon —
        //        can always catch up across the retained logs. This is what
        //        keeps concurrent compaction on a last-write-wins backend
        //        data-safe: the recent logs a peer still needs survive even if a
        //        lower-horizon snapshot wins the race.
        //      - `snapshot_ts - retention` CAPS how far one behind device holds
        //        the cutoff back. A device offline past the retention window is
        //        abandoned — its old logs are GC'd and it consumes the snapshot
        //        on return — so the folder can't grow without bound behind a
        //        single chronically-offline device (the original report).
        //    `last_seen_log` carries each device's HELD HORIZON
        //    (`max(cursor, own-newest-log)`, stamped by the heartbeat), so this
        //    reasons about real coverage, not a wall-clock heartbeat.
        // Floor the retention at 1 day: a `0` (or accidentally tiny) value
        // would push `retention_floor` up to `snapshot_ts`, overriding the
        // conservative `min_device_held` term and re-opening the
        // concurrent-compaction data-loss window (a compactor would GC recent
        // logs a concurrent lower-horizon peer still needs). One day is far
        // larger than any concurrent-compaction interval, so it keeps the
        // floor effective while still letting the user tune the pile-up bound.
        let retention_days = self
            .read_pref_u32(PREF_GC_RETENTION_DAYS, DEFAULT_GC_RETENTION_DAYS)
            .max(1);
        // `checked_sub_signed` rather than `-`: an absurd retention pref (the
        // key is device-local with no UI validation) would otherwise underflow
        // the DateTime range and panic. A value so large the subtraction
        // saturates means "retain effectively forever" → fall back to MIN_UTC,
        // which collapses `safe_cutoff` to the conservative `min_device_held`
        // floor (GC only what every device already has) — the correct reading
        // of an enormous retention window.
        let retention_floor = snapshot_ts
            .checked_sub_signed(ChronoDuration::days(i64::from(retention_days)))
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        let min_device_held = meta
            .devices
            .values()
            .map(|d| d.last_seen_log)
            .min()
            .unwrap_or(snapshot_ts);
        // Clamp to <= snapshot_ts: never GC a log the snapshot doesn't fold in.
        let safe_cutoff = min_device_held.max(retention_floor).min(snapshot_ts);

        // 4. Publish the GC high-water mark MONOTONICALLY (a compaction only
        //    ever raises it). Deletions are permanent, so once logs below some
        //    horizon are gone they stay gone; a concurrent lower-horizon
        //    compactor must not lower the advertised mark below what a peer
        //    already deleted — devices gate their stale check on this value.
        let gc_horizon = meta.gc_horizon_or_min().max(safe_cutoff);
        meta.gc_horizon = Some(gc_horizon);

        // 5. Mark devices whose held horizon is below the GC high-water mark:
        //    the logs they'd need to catch up incrementally are gone, so they
        //    must consume the snapshot on return (the §19.10 gate). A device
        //    merely behind the SNAPSHOT but at/above `gc_horizon` is NOT flagged
        //    — the retained logs let it catch up normally. Keying staleness on
        //    what was actually GC'd (not the snapshot content horizon) is the
        //    over-fire fix. The local device was just upserted at
        //    `snapshot_ts >= gc_horizon`, so it's never flagged.
        for record in meta.devices.values_mut() {
            if record.last_seen_log < gc_horizon {
                record.stale = true;
                report.stale_devices += 1;
            }
        }

        adapter.push_meta(&meta).await?;
        report.snapshot_coverage = names;

        // 6. GC the now-redundant logs, listed afresh: a peer may have
        //    appended to a file since the listing above, and only what a
        //    file holds now decides (review round 1 of #130). Delete one only
        //    when BOTH hold:
        //      - its timestamp is STRICTLY below `safe_cutoff`. Strict `<`
        //        spares the freshest covered log for one round.
        //      - the snapshot actually COVERS it: every event in it is held —
        //        our own, or applied. Not its name against our cursor: a
        //        session file named before the cursor may have grown after
        //        our last fetch, and deleting it would lose those events for
        //        the whole dataset. Never an empty file or one that does not
        //        parse.
        //    A failed listing skips the GC: the snapshot is out already.
        let current = match adapter.fetch_new_logs(&DeviceCursor::epoch()).await {
            Ok(logs) => logs,
            Err(err) => {
                warn!(?err, "couldn't list the logs again; no GC this time");
                Vec::new()
            }
        };
        for log in current {
            if log.name.timestamp >= safe_cutoff {
                continue;
            }
            if Coverage::deletable(&log, &self.local_device_id, self.store.as_ref())? {
                match adapter.delete_log(&log.name).await {
                    Ok(()) => report.deleted_logs += 1,
                    Err(err) => {
                        warn!(
                            log = %log.name.to_filename(),
                            ?err,
                            "compactor failed to delete redundant log",
                        );
                        report.failed_deletes += 1;
                    }
                }
            }
        }

        // 6. Reset local counters. Use `set` to "0" rather than
        //    `delete` so the next read returns a clean 0 without
        //    re-checking the absent-key path.
        let _ = self.store.set_pref(PREF_LOGS_SINCE_SNAPSHOT, "0");
        let _ = self.store.set_pref(PREF_BYTES_SINCE_SNAPSHOT, "0");

        info!(
            snapshot_ts = %snapshot_ts,
            gc_horizon = %gc_horizon,
            deleted = report.deleted_logs,
            stale = report.stale_devices,
            names,
            held = coverage.held.len(),
            "compaction complete",
        );
        Ok(report)
    }

    /// Bump the local "logs since last snapshot" counter by one
    /// and the byte counter by `bytes`. Called by the orchestrator
    /// after every successful `push_log` so the threshold check
    /// keeps up to date.
    pub fn record_pushed_log(&self, bytes: usize) {
        let new_logs = self.read_counter_u64(PREF_LOGS_SINCE_SNAPSHOT) + 1;
        let new_bytes = self.read_counter_u64(PREF_BYTES_SINCE_SNAPSHOT) + bytes as u64;
        let _ = self
            .store
            .set_pref(PREF_LOGS_SINCE_SNAPSHOT, &new_logs.to_string());
        let _ = self
            .store
            .set_pref(PREF_BYTES_SINCE_SNAPSHOT, &new_bytes.to_string());
    }

    fn read_pref_u32(&self, key: &str, default: u32) -> u32 {
        self.store
            .get_pref(key)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(default)
    }

    fn read_pref_u64(&self, key: &str, default: u64) -> u64 {
        self.store
            .get_pref(key)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(default)
    }

    /// Whether this device's applied record is exact
    /// ([`SYNC_COVERAGE_EXACT_SINCE_PREF_KEY`]). A record that does not read
    /// is not: naming nothing is the safe side.
    fn record_exact(&self) -> bool {
        matches!(
            self.store.get_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY),
            Ok(Some(_))
        )
    }

    fn read_counter_u64(&self, key: &str) -> u64 {
        self.store
            .get_pref(key)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    }

    /// Read a persisted RFC 3339 timestamp pref, or the `MIN_UTC` floor
    /// when absent/unparseable. Shared by the cursor + own-newest-log
    /// reads that anchor the content-bounded snapshot timestamp.
    ///
    /// A failed READ is an error, not a floor. `MIN_UTC` means "this device
    /// holds no content yet", which is a claim about the dataset made by a
    /// device that could not read how far it had actually got.
    ///
    /// On a dataset whose meta already carries a `gc_horizon`, the stale guard
    /// above catches that and refuses anyway — so the window this closes is the
    /// narrower one where it does not: a dataset with no horizon yet, where the
    /// floor takes the brand-new-dataset branch and stamps a snapshot as
    /// covering everything. Refusing costs one skipped round, and the counters
    /// are untouched, so the next round retries.
    fn read_ts_pref(&self, key: &str) -> SyncResult<DateTime<Utc>> {
        let raw = self
            .store
            .get_pref(key)
            .map_err(|err| sync_core::SyncError::internal(format!("read {key}: {err}")))?;
        Ok(raw
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::MIN_UTC))
    }

    /// This device's foreign fetch cursor — the newest foreign log it has
    /// applied. Bounds which foreign logs the GC may delete (only those at
    /// or below it are in the snapshot we built).
    fn read_cursor(&self) -> SyncResult<DateTime<Utc>> {
        self.read_ts_pref(SYNC_CURSOR_PREF_KEY)
    }

    /// This device's newest own-written log timestamp. Together with the
    /// cursor it bounds `snapshot_ts` to real, held content.
    fn read_own_newest_log(&self) -> SyncResult<DateTime<Utc>> {
        self.read_ts_pref(SYNC_OWN_NEWEST_LOG_PREF_KEY)
    }

    /// Inspect a snapshot we just generated and surface the size
    /// counters the report wants — no actual mutation.
    fn snapshot_size_counters(&self, snapshot: &sync_core::Snapshot) -> SnapshotApplyOutcome {
        // We could re-parse the body here, but the dump we built
        // 50 lines ago already has the counts. Simplest:
        // re-deserialise into the typed shape and count.
        let body: AperioSnapshotBody =
            serde_json::from_value(snapshot.body.clone()).unwrap_or_default();
        SnapshotApplyOutcome {
            rows_applied: body.dump.calendars.len()
                + body.dump.events.len()
                + body.dump.task_lists.len()
                + body.dump.tasks.len()
                + body.dump.color_labels.len(),
            rows_failed: 0,
            settings_applied: body.settings.len(),
            settings_failed: 0,
            accounts_applied: body.accounts.len(),
            accounts_failed: 0,
            covered_events: body.covered_event_ids.as_ref().map(Vec::len),
        }
    }
}

/// Delete every LOCAL pending log file stamped before `cut`. After a
/// snapshot has been pushed, those files are folded into it, so the
/// orchestrator must not re-upload them. Leaving them was the bug: the
/// compactor only ever dropped the single just-rotated file, so older
/// leftovers (e.g. from a prior crash) were re-pushed on every round and
/// never disappeared. The live post-rotation session file is stamped
/// exactly `cut`, so the strict `< cut` comparison keeps it; non-log files
/// in the dir are left alone.
///
/// Peers take log files, not the snapshot: a file longer than its remote
/// copy (`remote_lengths`, plaintext bytes by filename) is uploaded first —
/// the end of the rotated session, written since its last push. One that
/// fails to upload stays, and the next round's push takes it.
async fn sweep_redundant_pending(
    pending_dir: &Path,
    cut: DateTime<Utc>,
    adapter: &dyn SyncAdapter,
    remote_lengths: &HashMap<String, usize>,
) {
    let mut entries = match tokio::fs::read_dir(pending_dir).await {
        Ok(rd) => rd,
        // No staging dir yet → nothing to sweep.
        Err(_) => return,
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(err) => {
                debug!(?err, "couldn't read pending dir during compaction sweep");
                break;
            }
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(parsed) = LogFileName::from_filename(name) else {
            continue; // not a session log file — leave it untouched
        };
        if parsed.timestamp < cut {
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    debug!(name = name, ?err, "couldn't read pending log; keeping it");
                    continue;
                }
            };
            let uploaded = remote_lengths
                .get(name)
                .is_some_and(|remote| *remote >= bytes.len());
            if !bytes.is_empty() && !uploaded {
                let log = LogFile {
                    name: parsed,
                    bytes,
                };
                if let Err(err) = adapter.push_log(&log).await {
                    warn!(
                        name = name,
                        ?err,
                        "couldn't upload the end of a pending log; keeping it"
                    );
                    continue;
                }
            }
            match tokio::fs::remove_file(&path).await {
                Ok(()) => debug!(name = name, "swept pending log folded into snapshot"),
                Err(err) => debug!(name = name, ?err, "couldn't sweep redundant pending log"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeSecrets, FakeStore};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use sync_core::{LogFile, LogFileName, Snapshot};
    use tempfile::TempDir;

    /// In-memory fake adapter used by the compactor tests. Tracks
    /// pushed snapshots, meta, and logs; `delete_log` removes from
    /// the in-memory list.
    struct FakeAdapter {
        meta: Mutex<Option<MetaJson>>,
        snapshot: Mutex<Option<Snapshot>>,
        logs: Mutex<Vec<LogFile>>,
        /// Every log pushed, in order.
        pushed: Mutex<Vec<LogFile>>,
        /// Pushing a log fails.
        fail_pushes: Mutex<bool>,
        /// A peer's file as it reads once the snapshot is out: it grew while
        /// this device compacted.
        grow_on_snapshot: Mutex<Option<LogFile>>,
    }

    impl FakeAdapter {
        fn new() -> Self {
            Self {
                meta: Mutex::new(None),
                snapshot: Mutex::new(None),
                logs: Mutex::new(Vec::new()),
                pushed: Mutex::new(Vec::new()),
                fail_pushes: Mutex::new(false),
                grow_on_snapshot: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl SyncAdapter for FakeAdapter {
        async fn test_connection(&self) -> SyncResult<()> {
            Ok(())
        }
        async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
            Ok(self.meta.lock().unwrap().clone())
        }
        async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
            *self.meta.lock().unwrap() = Some(meta.clone());
            Ok(())
        }
        async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
            Ok(self
                .logs
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.name.timestamp > since.last_seen_log)
                .cloned()
                .collect())
        }
        async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
            if *self.fail_pushes.lock().unwrap() {
                return Err(sync_core::SyncError::internal("offline"));
            }
            self.pushed.lock().unwrap().push(log.clone());
            let mut logs = self.logs.lock().unwrap();
            logs.retain(|l| l.name != log.name);
            logs.push(log.clone());
            Ok(())
        }
        async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
            Ok(self.snapshot.lock().unwrap().clone())
        }
        async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
            *self.snapshot.lock().unwrap() = Some(snapshot.clone());
            if let Some(grown) = self.grow_on_snapshot.lock().unwrap().take() {
                let mut logs = self.logs.lock().unwrap();
                logs.retain(|l| l.name != grown.name);
                logs.push(grown);
            }
            Ok(())
        }
        async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
            let mut logs = self.logs.lock().unwrap();
            logs.retain(|l| l.name.to_filename() != name.to_filename());
            Ok(())
        }
        async fn push_sound_asset(
            &self,
            _hash: &str,
            _extension: &str,
            _bytes: &[u8],
        ) -> SyncResult<()> {
            Ok(())
        }
        async fn fetch_sound_asset(
            &self,
            _hash: &str,
            _extension: &str,
        ) -> SyncResult<Option<Vec<u8>>> {
            Ok(None)
        }
    }

    /// Build a compactor over an in-memory [`FakeStore`] (no DB, no
    /// keychain). The returned store handle lets a test seed thresholds
    /// and read back the post-compaction counters. The snapshot rows are
    /// empty — these tests exercise the compaction algorithm (snapshot
    /// push, meta update, log GC, counter reset), not snapshot contents.
    fn build_compactor() -> (Arc<FakeStore>, Compactor) {
        let store = Arc::new(FakeStore::default());
        let store_dyn: Arc<dyn SyncStore> = store.clone();
        let builder = Arc::new(SnapshotBuilder::new(
            store_dyn.clone(),
            Arc::new(FakeSecrets::default()),
            "1.0.0-test",
        ));
        let compactor = Compactor::new(
            store_dyn,
            builder,
            DeviceId::from_string("dev-this".into()),
            "1.0.0-test",
            None,
            None,
        );
        (store, compactor)
    }

    #[tokio::test]
    async fn compact_now_pushes_snapshot_and_meta() {
        let (_store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        let report = compactor.compact_now(&adapter).await.unwrap();
        assert!(report.snapshot_timestamp.is_some());
        assert!(adapter.snapshot.lock().unwrap().is_some());
        let meta = adapter.meta.lock().unwrap().clone().unwrap();
        // The compactor's own device is now in the registry.
        assert!(meta
            .device(&DeviceId::from_string("dev-this".into()))
            .is_some());
        // snapshot_timestamp on meta matches what we pushed.
        let snap_ts = adapter
            .snapshot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .metadata
            .snapshot_timestamp;
        assert_eq!(meta.snapshot_timestamp, snap_ts);
    }

    #[tokio::test]
    async fn compact_refuses_when_the_content_horizon_cant_be_read() {
        // An unreadable cursor used to collapse to the MIN_UTC floor, which
        // reads as "brand new dataset" and stamps the snapshot `now - 1s` —
        // advertising coverage of foreign logs this device never applied. The
        // round must fail instead, leaving the remote snapshot untouched.
        let (store, compactor) = build_compactor();
        store
            .prefs
            .lock()
            .unwrap()
            .insert(SYNC_CURSOR_PREF_KEY.into(), Utc::now().to_rfc3339());
        store
            .failing_pref_reads
            .lock()
            .unwrap()
            .insert(SYNC_CURSOR_PREF_KEY.into());

        let adapter = FakeAdapter::new();
        assert!(compactor.compact_now(&adapter).await.is_err());
        assert!(
            adapter.snapshot.lock().unwrap().is_none(),
            "no snapshot may be published on an unreadable horizon",
        );
        assert!(adapter.meta.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn compact_bumps_schema_and_min_app_version_on_upgrade() {
        // §19.13: a dataset written by an older wire format gets its
        // schema_version raised + min_app_version set to this app's version
        // when we compact (the snapshot we just generated is the new format).
        let (_store, compactor) = build_compactor(); // app_version "1.0.0-test"
        let adapter = FakeAdapter::new();
        let mut old = sync_core::MetaJson::fresh("0.9.0");
        old.schema_version = sync_core::SCHEMA_VERSION.saturating_sub(1);
        old.min_app_version = "0.9.0".into();
        *adapter.meta.lock().unwrap() = Some(old);

        compactor.compact_now(&adapter).await.unwrap();

        let meta = adapter.meta.lock().unwrap().clone().unwrap();
        assert_eq!(meta.schema_version, sync_core::SCHEMA_VERSION);
        assert_eq!(meta.min_app_version, "1.0.0-test");
    }

    #[tokio::test]
    async fn compact_preserves_min_app_version_when_schema_already_current() {
        // No schema upgrade → the existing min_app_version must NOT be raised.
        let (_store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        let mut current = sync_core::MetaJson::fresh("0.5.0");
        current.schema_version = sync_core::SCHEMA_VERSION;
        current.min_app_version = "0.5.0".into();
        *adapter.meta.lock().unwrap() = Some(current);

        compactor.compact_now(&adapter).await.unwrap();

        let meta = adapter.meta.lock().unwrap().clone().unwrap();
        assert_eq!(meta.schema_version, sync_core::SCHEMA_VERSION);
        assert_eq!(meta.min_app_version, "0.5.0");
    }

    #[tokio::test]
    async fn compact_now_deletes_covered_pre_snapshot_logs() {
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        // Our fetch cursor sits at "now" — we've applied every foreign log
        // up to here, so the snapshot we build covers them and the GC may
        // delete them. (Without a cursor the coverage guard would refuse to
        // delete foreign logs, since they'd look unapplied.)
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();

        // Pre-populate two logs from another device. The "old" one is well
        // in the past (before the snapshot horizon AND at/under our cursor →
        // covered → deletable); the "fresh" one is well in the future (newer
        // than the snapshot, so it must survive).
        let old_ts: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        let fresh_ts: DateTime<Utc> = (Utc::now() + ChronoDuration::days(1)).with_timezone(&Utc);
        adapter
            .logs
            .lock()
            .unwrap()
            .extend([held_log(&store, old_ts), held_log(&store, fresh_ts)]);

        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        *adapter.meta.lock().unwrap() = Some(meta);

        let report = compactor.compact_now(&adapter).await.unwrap();
        // The old log should be gone; the fresh one stays.
        assert_eq!(report.deleted_logs, 1);
        let remaining = adapter.logs.lock().unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].name.timestamp > Utc::now());
    }

    #[tokio::test]
    async fn retention_bounds_logs_pinned_by_a_chronically_offline_device() {
        // A device offline LONGER than the retention window used to hold the
        // cutoff back to its old last_seen, so every newer log piled up
        // forever. Now the retention floor (snapshot_ts - 14d) caps how far it
        // pins: logs older than the window are GC'd (it consumes the snapshot
        // on return), while logs WITHIN the window are retained so a
        // briefly-behind peer could still catch up incrementally.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        // We're caught up to "now": the snapshot covers everything we GC.
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();

        // Two foreign logs, both covered by our cursor: one OLDER than the 14d
        // retention window (→ GC'd) and one WITHIN it (→ retained).
        let old_log: DateTime<Utc> = Utc::now() - ChronoDuration::days(20);
        let recent_log: DateTime<Utc> = Utc::now() - ChronoDuration::days(5);
        for ts in [old_log, recent_log] {
            adapter.logs.lock().unwrap().push(held_log(&store, ts));
        }

        // dev-other is 30 days offline — past the retention window, so it no
        // longer pins logs older than the window.
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now() - ChronoDuration::days(30),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        *adapter.meta.lock().unwrap() = Some(meta);

        let report = compactor.compact_now(&adapter).await.unwrap();

        // Only the beyond-retention log is GC'd; the within-window one stays.
        assert_eq!(report.deleted_logs, 1);
        let remaining = adapter.logs.lock().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name.timestamp, recent_log);
        let meta = adapter.meta.lock().unwrap().clone().unwrap();
        // The chronically-offline device is flagged stale (its horizon is
        // below the published gc_horizon), and gc_horizon was published.
        assert!(meta.gc_horizon.is_some());
        assert!(
            meta.device(&DeviceId::from_string("dev-other".into()))
                .unwrap()
                .stale,
            "a device offline past the retention window must be flagged stale",
        );
    }

    #[tokio::test]
    async fn a_briefly_behind_device_pins_its_logs_and_is_not_flagged_stale() {
        // The over-fire / concurrent-safety guard: a device only a few days
        // behind (WITHIN the retention window) keeps its un-consumed logs — the
        // conservative `min_device_held` floor protects them — and is NOT
        // flagged stale, so it catches up incrementally instead of resuming.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();

        // A log from 1 day ago — NEWER than the behind device's 3-day horizon,
        // so it's one of the logs that device still needs to apply.
        let log_ts: DateTime<Utc> = Utc::now() - ChronoDuration::days(1);
        adapter.logs.lock().unwrap().push(held_log(&store, log_ts));

        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now() - ChronoDuration::days(3),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        *adapter.meta.lock().unwrap() = Some(meta);

        let report = compactor.compact_now(&adapter).await.unwrap();

        // The 1-day log is ABOVE the behind device's 3-day horizon, so the
        // conservative floor keeps it — nothing GC'd, device not stale.
        assert_eq!(report.deleted_logs, 0);
        assert_eq!(adapter.logs.lock().unwrap().len(), 1);
        let meta = adapter.meta.lock().unwrap().clone().unwrap();
        assert!(
            !meta
                .device(&DeviceId::from_string("dev-other".into()))
                .unwrap()
                .stale,
            "a briefly-behind device within retention must NOT be flagged stale",
        );
    }

    #[tokio::test]
    async fn gc_spares_a_foreign_log_newer_than_our_cursor() {
        // The data-loss guard: a foreign log NEWER than our fetch cursor is
        // one we never applied (e.g. a delayed push that landed after this
        // round's fetch), so it is NOT in the snapshot we just built. Even
        // though its timestamp is below the snapshot horizon, deleting it
        // would lose those events for the whole dataset — it must survive.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        // Our cursor is 10 days back: we've applied foreign logs only up to
        // there. But our OWN newest log is ~now, so the content horizon
        // (snapshot_ts) is ~now — well above the foreign log below.
        store
            .set_pref(
                SYNC_CURSOR_PREF_KEY,
                &(Utc::now() - ChronoDuration::days(10)).to_rfc3339(),
            )
            .unwrap();
        store
            .set_pref(SYNC_OWN_NEWEST_LOG_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();

        // Foreign log from 5 days ago: BELOW the snapshot horizon (~now) but
        // ABOVE our cursor (10 days ago), never applied → not held → must
        // NOT be deleted.
        let uncovered: DateTime<Utc> = Utc::now() - ChronoDuration::days(5);
        adapter
            .logs
            .lock()
            .unwrap()
            .push(log_of(uncovered, "dev-other", 1).0);

        let report = compactor.compact_now(&adapter).await.unwrap();

        assert_eq!(
            report.deleted_logs, 0,
            "a foreign log newer than our cursor is unapplied → not in the snapshot → must survive",
        );
        assert_eq!(adapter.logs.lock().unwrap().len(), 1);
    }

    /// A log file of `count` real events minted by `device`, and their ids.
    fn log_of(ts: DateTime<Utc>, device: &str, count: usize) -> (LogFile, Vec<String>) {
        let device_id = DeviceId::from_string(device.into());
        let envelopes: Vec<sync_core::EventEnvelope> = (0..count)
            .map(|i| {
                sync_core::EventEnvelope::new(
                    device_id.clone(),
                    sync_core::SyncEvent::TaskUpdated(sync_core::EventPayload {
                        id: format!("task-{i}"),
                        fields: serde_json::json!({ "status": "completed" }),
                    }),
                )
            })
            .collect();
        let ids = envelopes.iter().map(|e| e.id.clone()).collect();
        let bytes = envelopes
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        (
            LogFile {
                name: LogFileName::new(ts, device_id),
                bytes,
            },
            ids,
        )
    }

    fn body_of(adapter: &FakeAdapter) -> AperioSnapshotBody {
        let snapshot = adapter.snapshot.lock().unwrap().clone().unwrap();
        serde_json::from_value(snapshot.body).unwrap()
    }

    /// A peer that has seen everything up to now, so the GC cutoff is not
    /// held back by it.
    fn meta_with_caught_up_peer() -> MetaJson {
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        meta
    }

    #[tokio::test]
    async fn names_the_events_it_holds_when_it_holds_every_one() {
        // DESIGN-sync-snapshot-coverage.md: own events and applied foreign
        // ones are held; an exact device holding every readable one names
        // them.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        mark_exact(&store);
        let recent = Utc::now() - ChronoDuration::hours(2);
        let (own, own_ids) = log_of(recent, "dev-this", 2);
        let (peer, peer_ids) = log_of(recent + ChronoDuration::minutes(5), "dev-other", 2);
        for id in &peer_ids {
            store.mark_event_applied(id).unwrap();
        }
        adapter.logs.lock().unwrap().extend([own, peer]);
        *adapter.meta.lock().unwrap() = Some(meta_with_caught_up_peer());

        let report = compactor.compact_now(&adapter).await.unwrap();

        let mut named = body_of(&adapter)
            .covered_event_ids
            .expect("names its events");
        named.sort();
        let mut expected: Vec<String> = own_ids.into_iter().chain(peer_ids).collect();
        expected.sort();
        assert_eq!(named, expected);
        assert!(report.snapshot_coverage);
    }

    #[tokio::test]
    async fn names_nothing_while_its_record_is_not_exact() {
        // Holding every event in every file is not enough: a device that
        // joined or resumed from a snapshot before it caught up may have had
        // rows reverted under its record (DESIGN-sync-snapshot-coverage.md).
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        let (own, _) = log_of(Utc::now() - ChronoDuration::hours(2), "dev-this", 2);
        adapter.logs.lock().unwrap().push(own);
        *adapter.meta.lock().unwrap() = Some(meta_with_caught_up_peer());

        let report = compactor.compact_now(&adapter).await.unwrap();

        assert_eq!(body_of(&adapter).covered_event_ids, None);
        assert!(!report.snapshot_coverage);
    }

    fn mark_exact(store: &FakeStore) {
        store
            .set_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
    }

    #[tokio::test]
    async fn names_nothing_when_a_peers_event_was_never_applied() {
        // A peer's file holding an event this exact device never applied —
        // the file grew since the last fetch: the snapshot names nothing.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        mark_exact(&store);
        let (peer, peer_ids) = log_of(Utc::now() - ChronoDuration::hours(1), "dev-other", 2);
        store.mark_event_applied(&peer_ids[0]).unwrap();
        adapter.logs.lock().unwrap().push(peer);
        *adapter.meta.lock().unwrap() = Some(meta_with_caught_up_peer());

        let report = compactor.compact_now(&adapter).await.unwrap();

        assert_eq!(body_of(&adapter).covered_event_ids, None);
        assert!(!report.snapshot_coverage);
    }

    #[tokio::test]
    async fn gc_keeps_a_file_named_before_the_cursor_whose_rest_was_never_applied() {
        // The laptop's case (Toni, 2026-10-27): a peer's session file named
        // long before this device's cursor, whose later events it never
        // applied. By name it looked covered and was deleted, losing those
        // events for the whole dataset; now only a file whose every event is
        // held goes.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        let old: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        let (grown, grown_ids) = log_of(old, "dev-other", 2);
        store.mark_event_applied(&grown_ids[0]).unwrap();
        let (applied, applied_ids) = log_of(old + ChronoDuration::hours(1), "dev-other", 1);
        store.mark_event_applied(&applied_ids[0]).unwrap();
        adapter.logs.lock().unwrap().extend([grown, applied]);
        *adapter.meta.lock().unwrap() = Some(meta_with_caught_up_peer());

        let report = compactor.compact_now(&adapter).await.unwrap();

        assert_eq!(report.deleted_logs, 1);
        let remaining = adapter.logs.lock().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].name.timestamp, old,
            "the file with an unapplied event stays"
        );
    }

    /// A peer's log file of one event at `ts`, recorded as applied: held.
    fn held_log(store: &FakeStore, ts: DateTime<Utc>) -> LogFile {
        let (log, ids) = log_of(ts, "dev-other", 1);
        for id in &ids {
            store.mark_event_applied(id).unwrap();
        }
        log
    }

    /// The first line of a log file: what was pushed before it grew.
    fn first_line(log: &LogFile) -> LogFile {
        LogFile {
            name: log.name.clone(),
            bytes: log.bytes.split(|b| *b == b'\n').next().unwrap().to_vec(),
        }
    }

    #[tokio::test]
    async fn gc_keeps_an_empty_or_unreadable_file() {
        // Review round 1 of #130: an empty read may be a file being written,
        // an unreadable one may hold what a newer version wrote.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        let old: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        for (secs, bytes) in [(0, Vec::new()), (60, b"{}".to_vec())] {
            adapter.logs.lock().unwrap().push(LogFile {
                name: LogFileName::new(
                    old + ChronoDuration::seconds(secs),
                    DeviceId::from_string("dev-other".into()),
                ),
                bytes,
            });
        }
        *adapter.meta.lock().unwrap() = Some(meta_with_caught_up_peer());

        let report = compactor.compact_now(&adapter).await.unwrap();

        assert_eq!(report.deleted_logs, 0);
        assert_eq!(adapter.logs.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn gc_reads_each_file_afresh_after_the_snapshot() {
        // A peer appends to its session while this device compacts: what the
        // file held before the snapshot is not what it holds now.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        let old: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        let (grown, ids) = log_of(old, "dev-other", 2);
        store.mark_event_applied(&ids[0]).unwrap();
        adapter.logs.lock().unwrap().push(first_line(&grown));
        *adapter.grow_on_snapshot.lock().unwrap() = Some(grown);
        *adapter.meta.lock().unwrap() = Some(meta_with_caught_up_peer());

        let report = compactor.compact_now(&adapter).await.unwrap();

        assert_eq!(
            report.deleted_logs, 0,
            "its new event was never applied here"
        );
        assert_eq!(adapter.logs.lock().unwrap().len(), 1);
    }

    /// A compactor that stages in `pending`, no writer.
    fn compactor_with_pending(store: &Arc<FakeStore>, pending: &Path) -> Compactor {
        let store: Arc<dyn SyncStore> = store.clone();
        let builder = Arc::new(SnapshotBuilder::new(
            Arc::clone(&store),
            Arc::new(FakeSecrets::default()),
            "1.0.0-test",
        ));
        Compactor::new(
            store,
            builder,
            DeviceId::from_string("dev-this".into()),
            "1.0.0-test",
            None,
            Some(pending.to_path_buf()),
        )
    }

    /// Two of this device's pending sessions from an hour ago: one pushed
    /// whole, one whose end was written after its last push. Returns them
    /// and their local paths.
    fn pending_sessions(adapter: &FakeAdapter, pending: &Path) -> [(LogFile, PathBuf); 2] {
        let at = Utc::now() - ChronoDuration::hours(1);
        let (whole, _) = log_of(at, "dev-this", 1);
        let (grown, _) = log_of(at + ChronoDuration::minutes(1), "dev-this", 2);
        adapter
            .logs
            .lock()
            .unwrap()
            .extend([whole.clone(), first_line(&grown)]);
        [whole, grown].map(|log| {
            let path = pending.join(log.name.to_filename());
            std::fs::write(&path, &log.bytes).unwrap();
            (log, path)
        })
    }

    #[tokio::test]
    async fn the_end_of_a_pending_session_is_uploaded_before_it_is_swept() {
        // Review round 1 of #130: peers take log files, not the snapshot —
        // what this device wrote since its last push reached nobody.
        let dir = TempDir::new().unwrap();
        let store = Arc::new(FakeStore::default());
        let compactor = compactor_with_pending(&store, dir.path());
        let adapter = FakeAdapter::new();
        let [(_, whole_path), (grown, grown_path)] = pending_sessions(&adapter, dir.path());

        compactor.compact_now(&adapter).await.unwrap();

        let pushed = adapter.pushed.lock().unwrap();
        assert_eq!(pushed.len(), 1, "the whole one is not pushed again");
        assert_eq!(
            pushed[0].name.to_filename(),
            grown.name.to_filename(),
            "the grown one"
        );
        assert_eq!(pushed[0].bytes, grown.bytes, "with its end");
        assert!(!whole_path.exists() && !grown_path.exists(), "both swept");
    }

    #[tokio::test]
    async fn a_pending_session_that_fails_to_upload_stays() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(FakeStore::default());
        let compactor = compactor_with_pending(&store, dir.path());
        let adapter = FakeAdapter::new();
        *adapter.fail_pushes.lock().unwrap() = true;
        let [(_, whole_path), (_, grown_path)] = pending_sessions(&adapter, dir.path());

        compactor.compact_now(&adapter).await.unwrap();

        assert!(grown_path.exists(), "the next round's push takes it");
        assert!(!whole_path.exists(), "nothing of it is missing remotely");
    }

    #[tokio::test]
    async fn snapshot_ts_is_bounded_to_held_content() {
        // The snapshot horizon must be max(own_newest_log, cursor) — real
        // held content — NOT now(). This is what lets a caught-up peer's held
        // horizon land exactly on it instead of forever a hair below.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        let cursor_ts = Utc::now() - ChronoDuration::days(3);
        let own_ts = Utc::now() - ChronoDuration::days(1); // newer than cursor
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &cursor_ts.to_rfc3339())
            .unwrap();
        store
            .set_pref(SYNC_OWN_NEWEST_LOG_PREF_KEY, &own_ts.to_rfc3339())
            .unwrap();

        let report = compactor.compact_now(&adapter).await.unwrap();
        let stamped: DateTime<Utc> = report.snapshot_timestamp.unwrap().parse().unwrap();
        // Whole-second granularity: the compactor truncates `now` but anchors
        // on the (already whole-second-ish) content horizon. Compare to the
        // max of the two content inputs.
        let expected = own_ts.max(cursor_ts);
        assert_eq!(
            stamped, expected,
            "snapshot_ts should equal max(own_newest, cursor), not now()",
        );
        // And it must be safely in the past (below now), so post-rotation
        // edits sort after it.
        assert!(stamped < Utc::now());
    }

    #[tokio::test]
    async fn gc_strict_less_spares_the_log_exactly_at_the_snapshot_horizon() {
        // Strict `<` (not `<=`) leaves the freshest covered log on the remote
        // for one more round. That grace is what keeps a concurrent compactor
        // whose lower-horizon meta wins last-write-wins from having already
        // deleted a log a peer still sitting at that horizon needs. A log one
        // second below the horizon is still folded away normally.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        let horizon: DateTime<Utc> = Utc::now() - ChronoDuration::days(2);
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &horizon.to_rfc3339())
            .unwrap();
        // No own logs → snapshot_ts == cursor == horizon.
        let at_horizon = horizon;
        let below_horizon = horizon - ChronoDuration::seconds(1);
        for ts in [at_horizon, below_horizon] {
            adapter.logs.lock().unwrap().push(held_log(&store, ts));
        }

        let report = compactor.compact_now(&adapter).await.unwrap();
        assert_eq!(report.deleted_logs, 1, "only the below-horizon log folds");
        let remaining = adapter.logs.lock().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].name.timestamp, at_horizon,
            "the log exactly at the snapshot horizon must survive (strict `<`)",
        );
    }

    #[tokio::test]
    async fn compact_refuses_when_this_device_is_behind_the_gc_horizon() {
        // The manual "compact now" command bypasses the §19.10 backstop, so the
        // compactor must self-guard: a device whose content horizon is below
        // the published gc_horizon would push a snapshot covering less than the
        // dataset claims to have GC'd. It must instead surface StaleDevice (the
        // resume flow) and NOT overwrite the snapshot/meta.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        // This device is 20 days behind; the dataset has GC'd up to 5 days ago.
        let cursor_ts = Utc::now() - ChronoDuration::days(20);
        let gc = Utc::now() - ChronoDuration::days(5);
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &cursor_ts.to_rfc3339())
            .unwrap();

        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.snapshot_timestamp = gc;
        meta.gc_horizon = Some(gc);
        *adapter.meta.lock().unwrap() = Some(meta);

        let err = compactor.compact_now(&adapter).await.unwrap_err();
        assert!(
            matches!(err, sync_core::SyncError::StaleDevice { .. }),
            "a behind device's manual compaction must surface StaleDevice, got {err:?}",
        );
        // It must NOT have overwritten the snapshot with its partial content.
        assert!(
            adapter.snapshot.lock().unwrap().is_none(),
            "no snapshot should have been pushed by a refused compaction",
        );
        // And gc_horizon stays where it was (not lowered).
        let meta_after = adapter.meta.lock().unwrap().clone().unwrap();
        assert_eq!(meta_after.gc_horizon, Some(gc));
    }

    #[tokio::test]
    async fn absurd_retention_pref_does_not_panic() {
        // The retention pref is device-local with no UI validation; a garbage
        // value must not panic the DateTime subtraction. A saturating retention
        // collapses to the conservative floor rather than crashing.
        let (store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();
        store
            .set_pref(SYNC_CURSOR_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        store
            .set_pref(PREF_GC_RETENTION_DAYS, &u32::MAX.to_string())
            .unwrap();
        // Must return Ok (no panic) — the subtraction saturates to MIN_UTC.
        let report = compactor.compact_now(&adapter).await.unwrap();
        assert!(report.snapshot_timestamp.is_some());
    }

    #[tokio::test]
    async fn should_compact_fires_on_log_count_threshold() {
        let (store, compactor) = build_compactor();
        // Crank the threshold down so we don't need 1000 fake
        // pushes.
        store.set_pref(PREF_MAX_LOGS, "3").unwrap();
        // Simulate four pushes since the last snapshot.
        compactor.record_pushed_log(100);
        compactor.record_pushed_log(100);
        compactor.record_pushed_log(100);
        compactor.record_pushed_log(100);

        let adapter = FakeAdapter::new();
        // No meta yet → age check returns false; log threshold
        // does the trigger.
        assert!(compactor.should_compact(&adapter, None).await.unwrap());
    }

    #[tokio::test]
    async fn record_pushed_log_resets_after_compaction() {
        let (store, compactor) = build_compactor();
        compactor.record_pushed_log(500);
        compactor.record_pushed_log(500);
        let adapter = FakeAdapter::new();
        compactor.compact_now(&adapter).await.unwrap();
        // Counters cleared.
        assert_eq!(
            store.get_pref(PREF_LOGS_SINCE_SNAPSHOT).unwrap().as_deref(),
            Some("0"),
        );
        assert_eq!(
            store
                .get_pref(PREF_BYTES_SINCE_SNAPSHOT)
                .unwrap()
                .as_deref(),
            Some("0"),
        );
    }

    #[tokio::test]
    async fn compact_now_rotates_writer_and_drops_the_pre_compaction_log() {
        let dir = TempDir::new().unwrap();
        let store: Arc<dyn SyncStore> = Arc::new(FakeStore::default());
        let builder = Arc::new(SnapshotBuilder::new(
            store.clone(),
            Arc::new(FakeSecrets::default()),
            "1.0.0-test",
        ));
        let device = DeviceId::from_string("dev-rot".into());

        // A writer staging into <dir>/sync/log/pending/. Its session
        // started a minute ago, so its file is clearly "pre-compaction".
        let session_at = Utc::now() - ChronoDuration::seconds(60);
        let writer = EventLogWriter::spawn_with_kick(
            dir.path().to_path_buf(),
            device.clone(),
            None,
            session_at,
        );
        writer.append(sync_core::SyncEvent::EventDeleted(sync_core::IdPayload {
            id: "x".into(),
        }));

        let pending = dir.path().join("sync").join("log").join("pending");
        let pre_path = pending.join(LogFileName::new(session_at, device.clone()).to_filename());
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if pre_path.exists() {
                break;
            }
        }
        assert!(
            pre_path.exists(),
            "pre-compaction session file should exist"
        );

        let compactor = Compactor::new(
            Arc::clone(&store),
            builder,
            device.clone(),
            "1.0.0-test",
            Some(Arc::clone(&writer)),
            Some(pending.clone()),
        );
        let adapter = FakeAdapter::new();
        compactor.compact_now(&adapter).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // The pre-compaction log is gone — its mutations are in the
        // snapshot, so re-uploading them would be redundant.
        assert!(
            !pre_path.exists(),
            "pre-compaction log should be deleted after compaction",
        );

        // The fresh session file sorts strictly AFTER the snapshot, so a
        // device that consumes the snapshot (cursor → snapshot ts) still
        // fetches it instead of skipping it as "older than the snapshot".
        let snap_ts = adapter
            .snapshot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .metadata
            .snapshot_timestamp;
        let new_file = std::fs::read_dir(&pending)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path() != pre_path)
            .expect("a fresh post-compaction session file");
        let parsed = LogFileName::from_filename(&new_file.file_name().to_string_lossy())
            .expect("filename parseable");
        assert!(
            parsed.timestamp > snap_ts,
            "new session file ({}) must sort after the snapshot ({})",
            parsed.timestamp,
            snap_ts,
        );
    }

    #[tokio::test]
    async fn compact_now_sweeps_old_leftover_pending_logs() {
        // The actual cross-device bug: OLD pending files (e.g. left behind
        // by a prior crash) are NOT the current session's rotated file, so
        // the previous single-file cleanup left them — and every push
        // re-uploaded them forever. Compaction must sweep ALL pending files
        // the snapshot covers, while keeping the live post-rotation file.
        let dir = TempDir::new().unwrap();
        let store: Arc<dyn SyncStore> = Arc::new(FakeStore::default());
        let builder = Arc::new(SnapshotBuilder::new(
            store.clone(),
            Arc::new(FakeSecrets::default()),
            "1.0.0-test",
        ));
        let device = DeviceId::from_string("dev-sweep".into());
        let pending = dir.path().join("sync").join("log").join("pending");
        tokio::fs::create_dir_all(&pending).await.unwrap();

        // A non-empty leftover log from a long-gone session.
        let old_ts: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        let leftover = pending.join(LogFileName::new(old_ts, device.clone()).to_filename());
        tokio::fs::write(&leftover, b"{\"x\":1}\n").await.unwrap();

        // The current session's live file.
        let session_at = Utc::now() - ChronoDuration::seconds(5);
        let writer = EventLogWriter::spawn_with_kick(
            dir.path().to_path_buf(),
            device.clone(),
            None,
            session_at,
        );
        writer.append(sync_core::SyncEvent::EventDeleted(sync_core::IdPayload {
            id: "y".into(),
        }));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let compactor = Compactor::new(
            Arc::clone(&store),
            builder,
            device.clone(),
            "1.0.0-test",
            Some(Arc::clone(&writer)),
            Some(pending.clone()),
        );
        let adapter = FakeAdapter::new();
        compactor.compact_now(&adapter).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // The old leftover (covered by the snapshot) is swept.
        assert!(
            !leftover.exists(),
            "old leftover pending log should be swept by compaction",
        );

        // Only the live post-compaction file remains, stamped after the
        // snapshot so a device consuming the snapshot still fetches it.
        let snap_ts = adapter
            .snapshot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .metadata
            .snapshot_timestamp;
        let remaining: Vec<_> = std::fs::read_dir(&pending)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(
            remaining.len(),
            1,
            "only the live post-compaction session file should remain",
        );
        let parsed =
            LogFileName::from_filename(&remaining[0].file_name().to_string_lossy()).unwrap();
        assert!(
            parsed.timestamp > snap_ts,
            "remaining file ({}) must sort after the snapshot ({})",
            parsed.timestamp,
            snap_ts,
        );
    }
}
