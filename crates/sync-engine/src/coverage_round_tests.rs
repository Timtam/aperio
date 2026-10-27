//! The round's one-time rebuild (DESIGN-sync-snapshot-coverage.md; Toni,
//! 2026-10-27). Once meta.json says the current snapshot names the events it
//! holds, a device whose record is not exact yet rebuilds from it — and only
//! once: a device that is exact, or a dataset whose snapshot names nothing,
//! leaves the round as it was. A failed rebuild must not fail the round.

use std::sync::{Arc, Mutex};

use adapter_local::LocalFsSyncAdapter;
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use sync_core::{
    DeviceId, EventEnvelope, IdPayload, LogFile, LogFileName, MetaJson, SyncAdapter, SyncError,
    SyncEvent, SyncResult,
};
use tempfile::TempDir;

use crate::test_support::{FakeSecrets, FakeStore};
use crate::{
    Compactor, EventLogApplier, SnapshotBuilder, SyncOrchestrator, SyncRoundHooks, SyncStore,
    SYNC_COVERAGE_EXACT_SINCE_PREF_KEY,
};

/// Counts rebuilds; a successful one marks the record exact, as the
/// platform's onboarding service does.
struct RecordingHooks {
    store: Arc<FakeStore>,
    rebuilds: Mutex<usize>,
    fail: bool,
}

#[async_trait::async_trait]
impl SyncRoundHooks for RecordingHooks {
    fn app_version(&self) -> String {
        "1.0.0-test".into()
    }

    async fn heartbeat(
        &self,
        _adapter: &dyn SyncAdapter,
        _last_seen_log: DateTime<Utc>,
        _round_meta: Option<&MetaJson>,
    ) -> SyncResult<()> {
        Ok(())
    }

    async fn rebuild_from_snapshot(&self, _adapter: &dyn SyncAdapter) -> SyncResult<()> {
        *self.rebuilds.lock().unwrap() += 1;
        if self.fail {
            return Err(SyncError::internal("offline"));
        }
        self.store
            .set_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
        Ok(())
    }
}

/// One device over a local-FS remote whose meta.json says the snapshot
/// names its events (`coverage`) or not.
async fn device(
    coverage: bool,
    fail: bool,
) -> (
    TempDir,
    SyncOrchestrator,
    Arc<RecordingHooks>,
    Arc<dyn SyncAdapter>,
) {
    let tmp = TempDir::new().expect("tempdir");
    let remote = tmp.path().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let adapter: Arc<dyn SyncAdapter> = Arc::new(LocalFsSyncAdapter::new(&remote));
    let mut meta = MetaJson::fresh("1.0.0-test");
    meta.snapshot_coverage = coverage;
    adapter.push_meta(&meta).await.unwrap();

    let device_id = DeviceId::from_string("dev-laptop".into());
    let pending = tmp.path().join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let fake = Arc::new(FakeStore::default());
    let store: Arc<dyn SyncStore> = fake.clone();
    let applier = Arc::new(EventLogApplier::new(
        Arc::clone(&store),
        Arc::new(FakeSecrets::default()),
        Arc::new(adapter_local::LocalAdapter::new(
            adapter_local::test_support::open_test_db(),
        )),
        device_id.clone(),
    ));
    let builder = Arc::new(SnapshotBuilder::new(
        Arc::clone(&store),
        Arc::new(FakeSecrets::default()),
        "1.0.0-test",
    ));
    let compactor = Arc::new(Compactor::new(
        Arc::clone(&store),
        builder,
        device_id.clone(),
        "1.0.0-test",
        None,
        None,
    ));
    let hooks = Arc::new(RecordingHooks {
        store: fake,
        rebuilds: Mutex::new(0),
        fail,
    });
    let orchestrator = SyncOrchestrator::new(
        store,
        pending,
        device_id,
        applier,
        hooks.clone(),
        compactor,
        Utc::now(),
    );
    orchestrator.configure(Arc::clone(&adapter));
    (tmp, orchestrator, hooks, adapter)
}

#[tokio::test]
async fn a_device_rebuilds_once_from_the_first_snapshot_that_names_its_events() {
    let (_tmp, orchestrator, hooks, _adapter) = device(true, false).await;
    orchestrator.sync_now().await.expect("round 1");
    orchestrator.sync_now().await.expect("round 2");
    assert_eq!(
        *hooks.rebuilds.lock().unwrap(),
        1,
        "once, then its record is exact"
    );
}

#[tokio::test]
async fn no_rebuild_while_the_snapshot_names_nothing() {
    let (_tmp, orchestrator, hooks, _adapter) = device(false, false).await;
    orchestrator.sync_now().await.expect("round");
    assert_eq!(*hooks.rebuilds.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_failed_rebuild_leaves_the_round_going_and_retries() {
    let (_tmp, orchestrator, hooks, _adapter) = device(true, true).await;
    orchestrator
        .sync_now()
        .await
        .expect("the round still succeeds");
    orchestrator.sync_now().await.expect("and the next one");
    assert_eq!(*hooks.rebuilds.lock().unwrap(), 2, "retried each round");
}

/// A peer's session file named at `secs`, holding one event.
fn session(device: &str, secs: i64) -> LogFile {
    let device_id = DeviceId::from_string(device.into());
    let envelope = EventEnvelope {
        id: format!("ev-{device}-{secs}"),
        device_id: device_id.clone(),
        timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
        event: SyncEvent::EventDeleted(IdPayload {
            id: format!("row-{secs}"),
        }),
    };
    LogFile {
        name: LogFileName::new(Utc.timestamp_opt(secs, 0).unwrap(), device_id),
        bytes: serde_json::to_vec(&envelope).unwrap(),
    }
}

/// A peer pushes a session late — offline, named before this device's
/// cursor. `exact` decides whether this device's record is exact.
async fn late_session_rounds(exact: bool) -> [usize; 3] {
    let (_tmp, orchestrator, hooks, adapter) = device(false, false).await;
    if exact {
        hooks
            .store
            .set_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &Utc::now().to_rfc3339())
            .unwrap();
    }
    adapter
        .push_log(&session("dev-desktop", 2_000))
        .await
        .unwrap();
    let first = orchestrator.sync_now().await.expect("round 1").fetched_logs;
    adapter
        .push_log(&session("dev-phone", 1_500))
        .await
        .unwrap();
    let late = orchestrator.sync_now().await.expect("round 2").fetched_logs;
    let quiet = orchestrator.sync_now().await.expect("round 3").fetched_logs;
    [first, late, quiet]
}

#[tokio::test]
async fn an_exact_device_fetches_a_session_pushed_late_below_its_cursor() {
    // DESIGN-sync-snapshot-coverage.md: by name it lay below the cursor and
    // reached nobody. Fetched once, then known.
    assert_eq!(late_session_rounds(true).await, [1, 1, 0]);
}

#[tokio::test]
async fn an_inexact_device_keeps_the_old_rule_until_it_is_rebuilt() {
    // Re-fetching would re-apply what its rows hold from a snapshot that
    // named nothing; the rebuild makes it exact first.
    assert_eq!(late_session_rounds(false).await, [1, 0, 0]);
}
