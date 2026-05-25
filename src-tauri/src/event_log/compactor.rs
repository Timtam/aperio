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
//! 3. Compute safe_cutoff = min(snapshot_ts, min_device_last_seen).
//!    Log files with timestamp < safe_cutoff are redundant for
//!    every known device.
//! 4. Delete those log files via adapter.delete_log.
//! 5. Mark devices whose last_seen_log < snapshot_ts as stale —
//!    they need to consume the snapshot on next sync.
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

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use sync_core::{DeviceCursor, DeviceId, DeviceRecord, MetaJson, SyncAdapter, SyncResult};
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::event_log::snapshot::{SnapshotApplyOutcome, SnapshotBuilder};
use crate::user_prefs::UserPrefsRepo;

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
    db: SharedConn,
    builder: Arc<SnapshotBuilder>,
    local_device_id: DeviceId,
    app_version: String,
}

impl Compactor {
    pub fn new(
        db: SharedConn,
        builder: Arc<SnapshotBuilder>,
        local_device_id: DeviceId,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            db,
            builder,
            local_device_id,
            app_version: app_version.into(),
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
        // 1. Build + push the snapshot.
        let snapshot = self.builder.build()?;
        let snapshot_ts = snapshot.metadata.snapshot_timestamp;
        adapter.push_snapshot(&snapshot).await?;

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
        meta.snapshot_timestamp = snapshot_ts;
        let device_name = UserPrefsRepo::new(&self.db)
            .get(crate::event_log::PREF_DEVICE_NAME)
            .ok()
            .flatten();
        meta.upsert_device(
            &self.local_device_id,
            DeviceRecord {
                name: device_name,
                last_seen_log: snapshot_ts,
                app_version: self.app_version.clone(),
                stale: false,
            },
        );

        // 3. Compute safe cutoff = min(snapshot_ts, every device's
        //    last_seen_log). Anything older than that is fold-
        //    safe.
        let min_device_seen = meta
            .devices
            .values()
            .map(|d| d.last_seen_log)
            .min()
            .unwrap_or(snapshot_ts);
        let safe_cutoff = snapshot_ts.min(min_device_seen);

        // 4. Mark stale devices (last_seen_log < snapshot_ts).
        for record in meta.devices.values_mut() {
            if record.last_seen_log < snapshot_ts {
                record.stale = true;
                report.stale_devices += 1;
            }
        }

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
        let prefs = UserPrefsRepo::new(&self.db);
        let _ = prefs.set(PREF_LOGS_SINCE_SNAPSHOT, "0");
        let _ = prefs.set(PREF_BYTES_SINCE_SNAPSHOT, "0");

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
        let prefs = UserPrefsRepo::new(&self.db);
        let new_logs = self.read_counter_u64(PREF_LOGS_SINCE_SNAPSHOT) + 1;
        let new_bytes = self.read_counter_u64(PREF_BYTES_SINCE_SNAPSHOT) + bytes as u64;
        let _ = prefs.set(PREF_LOGS_SINCE_SNAPSHOT, &new_logs.to_string());
        let _ = prefs.set(PREF_BYTES_SINCE_SNAPSHOT, &new_bytes.to_string());
    }

    fn read_pref_u32(&self, key: &str, default: u32) -> u32 {
        UserPrefsRepo::new(&self.db)
            .get(key)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(default)
    }

    fn read_pref_u64(&self, key: &str, default: u64) -> u64 {
        UserPrefsRepo::new(&self.db)
            .get(key)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(default)
    }

    fn read_counter_u64(&self, key: &str) -> u64 {
        UserPrefsRepo::new(&self.db)
            .get(key)
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
        let body: crate::event_log::snapshot::AperioSnapshotBody =
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use async_trait::async_trait;
    use cal_adapter_local::LocalAdapter;
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

    fn build_compactor() -> (TempDir, DbHandle, Compactor) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        let adapter = Arc::new(LocalAdapter::new(db.shared()));
        let builder = Arc::new(SnapshotBuilder::new(db.shared(), adapter, "1.0.0-test"));
        let compactor = Compactor::new(
            db.shared(),
            builder,
            DeviceId::from_string("dev-this".into()),
            "1.0.0-test",
        );
        (dir, db, compactor)
    }

    #[tokio::test]
    async fn compact_now_pushes_snapshot_and_meta() {
        let (_tmp, _db, compactor) = build_compactor();
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
    async fn compact_now_deletes_logs_older_than_safe_cutoff() {
        let (_tmp, _db, compactor) = build_compactor();
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
    async fn should_compact_fires_on_log_count_threshold() {
        let (_tmp, db, compactor) = build_compactor();
        // Crank the threshold down so we don't need 1000 fake
        // pushes.
        UserPrefsRepo::new(&db.shared())
            .set(PREF_MAX_LOGS, "3")
            .unwrap();
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
        let (_tmp, db, compactor) = build_compactor();
        compactor.record_pushed_log(500);
        compactor.record_pushed_log(500);
        let adapter = FakeAdapter::new();
        compactor.compact_now(&adapter).await.unwrap();
        // Counters cleared.
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        assert_eq!(
            prefs.get(PREF_LOGS_SINCE_SNAPSHOT).unwrap().as_deref(),
            Some("0"),
        );
        assert_eq!(
            prefs.get(PREF_BYTES_SINCE_SNAPSHOT).unwrap().as_deref(),
            Some("0"),
        );
    }
}
