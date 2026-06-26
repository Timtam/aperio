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
//!    the remote (replacing any prior snapshot.json).
//! 2. Update meta.json.snapshot_timestamp to the snapshot's
//!    timestamp; mark this device's heartbeat at the same point.
//! 3. Mark devices whose last_seen_log < snapshot_ts as stale —
//!    they consume the snapshot (not the old logs) on next sync,
//!    gated by the §19.10 stale check in the orchestrator.
//! 4. Compute safe_cutoff = min(snapshot_ts, last_seen_log of each
//!    NON-stale device). Stale/offline devices DON'T hold the cutoff
//!    back — the snapshot already supersedes every earlier log for
//!    them — so this resolves to snapshot_ts and a chronically-offline
//!    device can't keep the log files piling up. Log files older than
//!    safe_cutoff are redundant for every device.
//! 5. Delete those log files via adapter.delete_log.
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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use serde::Serialize;
use sync_core::{
    DeviceCursor, DeviceId, DeviceRecord, LogFileName, MetaJson, SyncAdapter, SyncResult,
};
use tracing::{debug, info, warn};

use crate::{
    AperioSnapshotBody, EventLogWriter, SnapshotApplyOutcome, SnapshotBuilder, SyncStore,
    PREF_DEVICE_NAME,
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

pub const DEFAULT_MAX_AGE_DAYS: u32 = 30;
pub const DEFAULT_MAX_LOGS: u32 = 1000;
pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024 * 1024;

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
    pub async fn should_compact(&self, adapter: &dyn SyncAdapter) -> SyncResult<bool> {
        // Age threshold uses meta.snapshot_timestamp as the
        // anchor. If meta is missing, treat the dataset as
        // brand-new — no compaction needed yet.
        let meta = adapter.fetch_meta().await?;
        if let Some(meta) = &meta {
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
        // `cut` is whole-second because log filenames are second-granular.
        let cut = Utc::now().with_nanosecond(0).unwrap_or_else(Utc::now);
        let snapshot_ts = cut - ChronoDuration::seconds(1);
        // Roll the live session file over to a fresh one stamped `cut`, so
        // the snapshot below captures everything the old file holds and
        // post-compaction edits land in a file NEWER than the snapshot.
        if let Some(writer) = &self.writer {
            let _ = writer.rotate(cut).await;
        }

        // 1. Build + push the snapshot, stamped just before the new file.
        let snapshot = self.builder.build_at(snapshot_ts)?;
        adapter.push_snapshot(&snapshot).await?;

        // The snapshot is now durable on the remote, so every LOCAL pending
        // file stamped before `cut` is fully covered by it and must not be
        // re-uploaded on the next push. Sweep ALL of them — not just the
        // file we just rotated away: old leftovers (e.g. from a prior crash
        // that left logs in `pending/`) would otherwise be re-pushed on
        // every round and never disappear. The live post-rotation file is
        // stamped `cut` itself, so the strict `< cut` check keeps it.
        if let Some(pending) = &self.pending_dir {
            sweep_redundant_pending(pending, cut).await;
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
                app_version: self.app_version.clone(),
                stale: false,
            },
        );

        // 3. Mark stale devices (last_seen_log < snapshot_ts). A behind/offline
        //    device hits the §19.10 stale gate on its next sync (orchestrator
        //    sees its own `stale` flag → `StaleDevice` BEFORE fetching) and
        //    consumes the fresh SNAPSHOT, not the per-round logs — so from here
        //    it neither needs nor should pin the pre-snapshot logs.
        for record in meta.devices.values_mut() {
            if record.last_seen_log < snapshot_ts {
                record.stale = true;
                report.stale_devices += 1;
            }
        }

        // 4. Compute safe cutoff = min(snapshot_ts, last_seen_log of every
        //    NON-stale device). Stale devices no longer hold the cutoff back:
        //    the snapshot already supersedes every log before snapshot_ts for
        //    them. Non-stale devices sit at/after snapshot_ts, so in practice
        //    the cutoff resolves to snapshot_ts — every pre-snapshot log becomes
        //    fold-safe and a chronically-offline device can't keep the folder
        //    growing without bound. (`unwrap_or` is a belt: the just-upserted
        //    local device is non-stale at snapshot_ts, so there's always one.)
        let min_device_seen = meta
            .devices
            .values()
            .filter(|d| !d.stale)
            .map(|d| d.last_seen_log)
            .min()
            .unwrap_or(snapshot_ts);
        let safe_cutoff = snapshot_ts.min(min_device_seen);

        adapter.push_meta(&meta).await?;

        // 5. Delete redundant logs. Fetch with `epoch()` so we
        //    see every file regardless of cursor.
        let all_logs = adapter.fetch_new_logs(&DeviceCursor::epoch()).await?;
        for log in all_logs {
            if log.name.timestamp < safe_cutoff {
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
            deleted = report.deleted_logs,
            stale = report.stale_devices,
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

    fn read_counter_u64(&self, key: &str) -> u64 {
        self.store
            .get_pref(key)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
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
        }
    }
}

/// Delete every LOCAL pending log file stamped before `cut`. After a
/// snapshot has been pushed, those files are fully covered by it, so the
/// orchestrator must not re-upload them. Leaving them was the bug: the
/// compactor only ever dropped the single just-rotated file, so older
/// leftovers (e.g. from a prior crash) were re-pushed on every round and
/// never disappeared. The live post-rotation session file is stamped
/// exactly `cut`, so the strict `< cut` comparison keeps it; non-log files
/// in the dir are left alone.
async fn sweep_redundant_pending(pending_dir: &Path, cut: DateTime<Utc>) {
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
    }

    impl FakeAdapter {
        fn new() -> Self {
            Self {
                meta: Mutex::new(None),
                snapshot: Mutex::new(None),
                logs: Mutex::new(Vec::new()),
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
            self.logs.lock().unwrap().push(log.clone());
            Ok(())
        }
        async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
            Ok(self.snapshot.lock().unwrap().clone())
        }
        async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
            *self.snapshot.lock().unwrap() = Some(snapshot.clone());
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
    async fn compact_now_deletes_logs_older_than_safe_cutoff() {
        let (_store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        // Pre-populate two logs from another device. The "old"
        // one is well in the past; the "fresh" one is well in the
        // future (so it's newer than the snapshot the compactor
        // about to build).
        let old_ts: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        let fresh_ts: DateTime<Utc> = (Utc::now() + ChronoDuration::days(1)).with_timezone(&Utc);
        adapter.logs.lock().unwrap().push(LogFile {
            name: LogFileName::new(old_ts, DeviceId::from_string("dev-other".into())),
            bytes: b"{}".to_vec(),
        });
        adapter.logs.lock().unwrap().push(LogFile {
            name: LogFileName::new(fresh_ts, DeviceId::from_string("dev-other".into())),
            bytes: b"{}".to_vec(),
        });

        // Pre-populate meta so the other device's last_seen_log
        // is recent enough that `safe_cutoff` doesn't fall back
        // to its cursor.
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now(),
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
    async fn stale_device_no_longer_pins_pre_snapshot_logs() {
        // A device offline longer than the compaction window used to hold
        // safe_cutoff back to its old last_seen_log, so every log newer than that
        // piled up indefinitely. Now it's marked stale (it consumes the snapshot
        // on return via the §19.10 gate), so the cutoff is the snapshot itself
        // and those pre-snapshot logs get GC'd.
        let (_store, compactor) = build_compactor();
        let adapter = FakeAdapter::new();

        // A log from 5 days ago — AFTER the offline device's last_seen (11 days
        // ago) but BEFORE the snapshot the compactor is about to build (~now).
        let log_ts: DateTime<Utc> = Utc::now() - ChronoDuration::days(5);
        adapter.logs.lock().unwrap().push(LogFile {
            name: LogFileName::new(log_ts, DeviceId::from_string("dev-other".into())),
            bytes: b"{}".to_vec(),
        });

        // The other device hasn't synced in 11 days.
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now() - ChronoDuration::days(11),
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        *adapter.meta.lock().unwrap() = Some(meta);

        let report = compactor.compact_now(&adapter).await.unwrap();

        // Old behaviour KEPT this log (cutoff pinned to 11 days ago); now it's
        // deleted (cutoff = snapshot_ts) and the offline device is marked stale.
        assert_eq!(report.deleted_logs, 1);
        assert!(adapter.logs.lock().unwrap().is_empty());
        let meta = adapter.meta.lock().unwrap().clone().unwrap();
        assert!(
            meta.device(&DeviceId::from_string("dev-other".into()))
                .unwrap()
                .stale
        );
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
        assert!(compactor.should_compact(&adapter).await.unwrap());
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
