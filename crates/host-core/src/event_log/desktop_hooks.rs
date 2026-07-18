//! Desktop implementation of `sync_engine::SyncRoundHooks` — the
//! platform-coordination steps the sync round invokes but doesn't own:
//! the `meta.json` heartbeat + app version (via [`OnboardingService`]),
//! the out-of-band sound-asset sync (`crate::sound_assets`), the §20.8
//! device-name cache (`crate::device_names`), and the compaction audit
//! log (`crate::sync_log`). The round logic itself lives in the engine;
//! this is the desktop glue it calls back into.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sync_core::{MetaJson, SyncAdapter, SyncError, SyncResult};
use sync_engine::{CompactionReport, SyncRoundHooks};
use tracing::{info, warn};

use crate::db::SharedConn;
use crate::event_log::OnboardingService;

/// Holds the handles the desktop hooks need: the shared SQLite (for the
/// device-name cache, the sound-asset manifest, and the audit log), the
/// onboarding service (heartbeat + app version), and the local sound
/// store directory.
pub struct DesktopSyncRoundHooks {
    db: SharedConn,
    onboarding: Arc<OnboardingService>,
    sounds_dir: PathBuf,
}

impl DesktopSyncRoundHooks {
    pub fn new(db: SharedConn, onboarding: Arc<OnboardingService>, sounds_dir: PathBuf) -> Self {
        Self {
            db,
            onboarding,
            sounds_dir,
        }
    }
}

#[async_trait]
impl SyncRoundHooks for DesktopSyncRoundHooks {
    fn app_version(&self) -> String {
        self.onboarding.app_version().to_string()
    }

    async fn heartbeat(
        &self,
        adapter: &dyn SyncAdapter,
        last_seen_log: DateTime<Utc>,
        round_meta: Option<&sync_core::MetaJson>,
    ) -> SyncResult<()> {
        self.onboarding
            .heartbeat_meta(adapter, last_seen_log, round_meta)
            .await
    }

    async fn resume_from_stale(&self, adapter: &dyn SyncAdapter) -> SyncResult<()> {
        // §19.10 auto-resume: re-pull the snapshot, replay our pending logs over
        // it, and clear the stale flag. The report (rows applied etc.) isn't
        // needed by the round — it just needs to know the recovery succeeded.
        self.onboarding.resume_from_stale(adapter).await.map(|_| ())
    }

    async fn sync_sound_assets(&self, adapter: &dyn SyncAdapter) -> SyncResult<()> {
        let report = crate::sound_assets::sync_assets(&self.db, &self.sounds_dir, adapter).await?;
        if report.pushed > 0 || report.fetched > 0 || report.missing_on_remote > 0 {
            info!(
                pushed = report.pushed,
                fetched = report.fetched,
                missing = report.missing_on_remote,
                "sound asset sync",
            );
        }
        Ok(())
    }

    fn cache_device_names(&self, meta: &MetaJson) {
        let repo = crate::device_names::DeviceNamesRepo::new(&self.db);
        for (device_id, record) in &meta.devices {
            if let Err(err) = repo.upsert(device_id, record.name.as_deref()) {
                warn!(
                    device_id = %device_id,
                    ?err,
                    "couldn't cache device name from meta.json",
                );
            }
        }
    }

    fn record_compaction(&self, result: &Result<CompactionReport, SyncError>, duration_ms: u64) {
        use crate::sync_log::{SyncLogCounters, SyncLogRepo, SyncTrigger};
        let (success, counters, error) = match result {
            Ok(report) => {
                let success = report.failed_deletes == 0;
                let error = if success {
                    None
                } else {
                    Some(format!(
                        "{} of {} log deletions failed",
                        report.failed_deletes,
                        report.deleted_logs + report.failed_deletes,
                    ))
                };
                (
                    success,
                    SyncLogCounters {
                        pushed_logs: None,
                        fetched_logs: None,
                        applied: Some(u32::try_from(report.deleted_logs).unwrap_or(u32::MAX)),
                        conflicts: None,
                    },
                    error,
                )
            }
            Err(err) => (false, SyncLogCounters::default(), Some(err.to_string())),
        };
        let repo = SyncLogRepo::new(&self.db);
        if let Err(err) = repo.record(
            SyncTrigger::Compaction,
            success,
            &counters,
            Some(duration_ms),
            error.as_deref(),
        ) {
            warn!(?err, "couldn't persist compaction sync_log entry");
        }
    }
}
