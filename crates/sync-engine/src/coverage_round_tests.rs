//! The round's one-time catch-up (DESIGN-sync-snapshot-coverage.md; Toni,
//! 2026-10-30). A device whose applied record is not exact catches up once
//! over every log file still present; an exact device leaves the round as it
//! was. A failed catch-up must not fail the round. And an exact device
//! fetches what nobody would by name: a session pushed late below its
//! cursor, a live one named below the GC horizon that keeps growing.

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
    ApplyReport, Compactor, EventLogApplier, SnapshotBuilder, SyncOrchestrator, SyncRoundHooks,
    SyncStore, SYNC_COVERAGE_EXACT_SINCE_PREF_KEY,
};

/// Counts catch-ups; a successful one marks the record exact, as the
/// platform's onboarding service does.
struct RecordingHooks {
    store: Arc<FakeStore>,
    catch_ups: Mutex<usize>,
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

    async fn catch_up(&self, _adapter: &dyn SyncAdapter) -> SyncResult<ApplyReport> {
        *self.catch_ups.lock().unwrap() += 1;
        if self.fail {
            return Err(SyncError::internal("offline"));
        }
        mark_exact(&self.store);
        Ok(ApplyReport {
            applied: 3,
            settings_keys: vec!["appearance.darkMode".into()],
            day_markers_touched: true,
            ..ApplyReport::default()
        })
    }
}

fn mark_exact(store: &FakeStore) {
    store
        .set_pref(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &Utc::now().to_rfc3339())
        .unwrap();
}

/// One device over a local-FS remote holding `meta`. `fail` makes its
/// catch-up fail, so it stays as exact as the test sets it.
async fn device(
    meta: MetaJson,
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
        catch_ups: Mutex::new(0),
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

fn fresh_meta() -> MetaJson {
    MetaJson::fresh("1.0.0-test")
}

#[tokio::test]
async fn an_inexact_device_catches_up_once() {
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    orchestrator.sync_now().await.expect("round 1");
    orchestrator.sync_now().await.expect("round 2");
    assert_eq!(
        *hooks.catch_ups.lock().unwrap(),
        1,
        "once, then its record is exact"
    );
}

#[tokio::test]
async fn what_the_catch_up_applied_reaches_the_rounds_report() {
    // Review round 2 of #130: the report was dropped, so the platform never
    // re-read the settings or reloaded the views the catch-up changed.
    let (_tmp, orchestrator, _hooks, _adapter) = device(fresh_meta(), false).await;
    let report = orchestrator.sync_now().await.expect("round");
    assert_eq!(report.applied, 3);
    assert_eq!(
        report.settings_keys,
        vec!["appearance.darkMode".to_string()]
    );
    assert!(report.day_markers_touched);
}

#[tokio::test]
async fn an_exact_device_does_not_catch_up() {
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    orchestrator.sync_now().await.expect("round");
    assert_eq!(*hooks.catch_ups.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_failed_catch_up_leaves_the_round_going_and_retries() {
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), true).await;
    orchestrator
        .sync_now()
        .await
        .expect("the round still succeeds");
    orchestrator.sync_now().await.expect("and the next one");
    assert_eq!(*hooks.catch_ups.lock().unwrap(), 2, "retried each round");
}

/// A session file of `device` named at `secs`, holding `events` events.
fn session(device: &str, secs: i64, events: i64) -> LogFile {
    let device_id = DeviceId::from_string(device.into());
    let lines: Vec<String> = (0..events)
        .map(|n| {
            serde_json::to_string(&EventEnvelope {
                id: format!("ev-{device}-{secs}-{n}"),
                device_id: device_id.clone(),
                timestamp: Utc.timestamp_opt(secs + n, 0).unwrap(),
                event: SyncEvent::EventDeleted(IdPayload {
                    id: format!("row-{secs}-{n}"),
                }),
            })
            .unwrap()
        })
        .collect();
    LogFile {
        name: LogFileName::new(Utc.timestamp_opt(secs, 0).unwrap(), device_id),
        bytes: lines.join("\n").into_bytes(),
    }
}

/// A peer pushes a session late — offline, named before this device's
/// cursor. `exact` decides whether this device's record is exact; its
/// catch-up fails, so it stays that way.
async fn late_session_rounds(exact: bool) -> [usize; 3] {
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), true).await;
    if exact {
        mark_exact(&hooks.store);
    }
    adapter
        .push_log(&session("dev-desktop", 2_000, 1))
        .await
        .unwrap();
    let first = orchestrator.sync_now().await.expect("round 1").fetched_logs;
    adapter
        .push_log(&session("dev-phone", 1_500, 1))
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
async fn an_inexact_device_keeps_the_old_rule_until_it_caught_up() {
    assert_eq!(late_session_rounds(false).await, [1, 0, 0]);
}

#[tokio::test]
async fn a_live_session_named_below_the_gc_horizon_is_fetched_as_it_grows() {
    // Review round 1 of #130: the applied lengths were cut at the GC horizon,
    // so a peer's session still running from before it dropped out, and its
    // growth reached nobody — the laptop's gap, back. They are kept now.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), true).await;
    mark_exact(&hooks.store);
    adapter
        .push_log(&session("dev-desktop", 1_000, 1))
        .await
        .unwrap();
    adapter
        .push_log(&session("dev-phone", 5_000, 1))
        .await
        .unwrap();
    assert_eq!(
        orchestrator.sync_now().await.expect("round 1").fetched_logs,
        2
    );
    // A compaction elsewhere moves the horizon past the desktop's session,
    // which keeps running and grows.
    let mut meta = adapter.fetch_meta().await.unwrap().unwrap();
    meta.gc_horizon = Some(Utc.timestamp_opt(3_000, 0).unwrap());
    adapter.push_meta(&meta).await.unwrap();
    adapter
        .push_log(&session("dev-desktop", 1_000, 2))
        .await
        .unwrap();

    let report = orchestrator.sync_now().await.expect("round 2");
    assert_eq!(report.fetched_logs, 1, "the grown session");
    assert_eq!(report.applied, 1, "its new event");
}
