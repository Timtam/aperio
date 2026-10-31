//! The round's one-time catch-up (DESIGN-sync-snapshot-coverage.md; Toni,
//! 2026-10-30). A device whose applied record is not exact catches up once
//! over every log file still present; an exact device leaves the round as it
//! was. A failed catch-up must not fail the round. And an exact device
//! fetches what nobody would by name: a session pushed late below its
//! cursor, a live one named below the GC horizon that keeps growing.

use std::sync::{Arc, Mutex};

use adapter_local::LocalFsSyncAdapter;
use chrono::TimeZone;
use chrono::{DateTime, Timelike, Utc};
use sync_core::{
    DeviceId, EventEnvelope, IdPayload, LogFile, LogFileName, MetaJson, SyncAdapter, SyncError,
    SyncEvent, SyncResult,
};
use tempfile::TempDir;

use crate::test_support::{FakeSecrets, FakeStore};
use crate::{
    ApplyReport, Compactor, EventLogApplier, SnapshotBuilder, SyncOrchestrator, SyncRoundHooks,
    SyncStore, CATCH_UP_RETRY, SYNC_CATCH_UP_TRIED_AT_PREF_KEY, SYNC_COVERAGE_EXACT_SINCE_PREF_KEY,
    SYNC_UNREADABLE_LOGS_PREF_KEY,
};

/// Counts catch-ups; a successful one marks the record exact, as the
/// platform's onboarding service does.
struct RecordingHooks {
    store: Arc<FakeStore>,
    catch_ups: Mutex<usize>,
    fail: bool,
    /// A compaction elsewhere raises the GC horizon to this while the
    /// catch-up runs.
    horizon_meanwhile: Mutex<Option<DateTime<Utc>>>,
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

    async fn catch_up(&self, adapter: &dyn SyncAdapter) -> SyncResult<ApplyReport> {
        *self.catch_ups.lock().unwrap() += 1;
        let meanwhile = *self.horizon_meanwhile.lock().unwrap();
        if let Some(horizon) = meanwhile {
            let mut meta = adapter.fetch_meta().await?.unwrap_or_else(fresh_meta);
            meta.gc_horizon = Some(horizon);
            adapter.push_meta(&meta).await?;
        }
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
        horizon_meanwhile: Mutex::new(None),
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
async fn a_failed_catch_up_leaves_the_round_going_and_retries_an_hour_later() {
    // It fetches every log file: not again every round.
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), true).await;
    orchestrator
        .sync_now()
        .await
        .expect("the round still succeeds");
    orchestrator.sync_now().await.expect("and the next one");
    assert_eq!(*hooks.catch_ups.lock().unwrap(), 1, "not again right away");

    let earlier = Utc::now() - CATCH_UP_RETRY - chrono::Duration::minutes(1);
    hooks
        .store
        .set_pref(SYNC_CATCH_UP_TRIED_AT_PREF_KEY, &earlier.to_rfc3339())
        .unwrap();
    orchestrator.sync_now().await.expect("an hour later");
    assert_eq!(*hooks.catch_ups.lock().unwrap(), 2);
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
    // Not exact, so only its recorded length can fetch the file again.
    let (_tmp, orchestrator, _hooks, adapter) = device(fresh_meta(), true).await;
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
    // Its new event is written now, after the horizon.
    let mut grown = session("dev-desktop", 1_000, 1);
    let mut events = grown.into_envelopes().unwrap();
    let mut new_event = events[0].clone();
    new_event.id = "ev-dev-desktop-grown".into();
    new_event.timestamp = Utc.timestamp_opt(6_000, 0).unwrap();
    events.push(new_event);
    grown = LogFile::from_envelopes(grown.name.device_id.clone(), grown.name.timestamp, &events)
        .unwrap();
    adapter.push_log(&grown).await.unwrap();

    let report = orchestrator.sync_now().await.expect("round 2");
    assert_eq!(report.fetched_logs, 1, "the grown session");
    assert_eq!(report.applied, 1, "its new event");
}

/// One of this device's pending files: a password it set while E2E was on,
/// and a setting.
fn pending_with_a_secret(device: &str, secs: i64) -> LogFile {
    let device_id = DeviceId::from_string(device.into());
    let at = Utc.timestamp_opt(secs, 0).unwrap();
    let envelopes = [
        EventEnvelope {
            id: format!("ev-secret-{secs}"),
            device_id: device_id.clone(),
            timestamp: at,
            event: SyncEvent::CredentialSet(sync_core::CredentialPayload {
                account_id: "acc-1".into(),
                slot: "password".into(),
                secret: "hunter2".into(),
            }),
        },
        EventEnvelope {
            id: format!("ev-setting-{secs}"),
            device_id: device_id.clone(),
            timestamp: at,
            event: SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
                key: "appearance.darkMode".into(),
                value: serde_json::json!(true),
            }),
        },
    ];
    LogFile::from_envelopes(device_id, at, &envelopes).unwrap()
}

#[tokio::test]
async fn with_e2e_off_the_push_sends_no_secret() {
    // Review round 2 of #130: after an E2E downgrade purged the remote, the
    // pending files still held the credential events, and the first push
    // after a restart sent their secrets in the clear.
    let (tmp, orchestrator, hooks, adapter) = device(fresh_meta(), true).await;
    mark_exact(&hooks.store);
    let file = pending_with_a_secret("dev-laptop", 2_000);
    std::fs::write(
        tmp.path().join("pending").join(file.name.to_filename()),
        &file.bytes,
    )
    .unwrap();

    orchestrator.sync_now().await.expect("round");

    let remote = adapter
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
        .unwrap();
    assert_eq!(remote.len(), 1);
    let pushed = remote[0].into_envelopes().unwrap();
    assert_eq!(pushed.len(), 1, "the setting only");
    assert!(!String::from_utf8_lossy(&remote[0].bytes).contains("hunter2"));
}

#[tokio::test]
async fn a_finished_session_goes_once_pushed_and_the_live_one_stays() {
    // Review round 2 of #130: pending files from earlier app starts went up
    // again after every start, so a file the GC had deleted came back for an
    // exact device to take in as unknown.
    let (tmp, orchestrator, _hooks, adapter) = device(fresh_meta(), true).await;
    let pending = tmp.path().join("pending");
    let finished = session("dev-laptop", 2_000, 1);
    let live = session("dev-laptop", Utc::now().timestamp() + 60, 1);
    for file in [&finished, &live] {
        std::fs::write(pending.join(file.name.to_filename()), &file.bytes).unwrap();
    }

    let report = orchestrator.sync_now().await.expect("round");

    assert_eq!(report.pushed_logs, 2);
    assert!(!pending.join(finished.name.to_filename()).exists());
    assert!(pending.join(live.name.to_filename()).exists());
    let remote = adapter
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
        .unwrap();
    assert_eq!(remote.len(), 2, "both are on the remote");
}

#[tokio::test]
async fn an_ordinary_round_takes_a_late_change_older_than_the_gc_horizon() {
    // Review round 4 of #130: holding every change older than the horizon
    // dropped the offline work of a device away past the retention window —
    // and ordinary changes in a long session named before a peer's
    // compaction — on every device but the one that made them. The horizon
    // follows file names, not deleted files: such a change is weighed.
    let mut meta = fresh_meta();
    meta.gc_horizon = Some(Utc.timestamp_opt(3_000, 0).unwrap());
    let (_tmp, orchestrator, hooks, adapter) = device(meta, true).await;
    mark_exact(&hooks.store);
    hooks
        .store
        .set_pref(
            crate::SYNC_CURSOR_PREF_KEY,
            &Utc.timestamp_opt(4_000, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    adapter
        .push_log(&session("dev-desktop", 1_000, 1))
        .await
        .unwrap();

    let report = orchestrator.sync_now().await.expect("round");

    assert_eq!(report.fetched_logs, 1, "unknown, below the cursor");
    assert_eq!(report.applied, 1);
}

/// One file, unreadable to the parser `parser`.
fn unreadable_by(parser: u64) -> String {
    serde_json::to_string(&crate::UnreadableLogs {
        parser,
        files: std::collections::HashMap::from([("x.jsonl".to_string(), 1)]),
    })
    .unwrap()
}

fn applied_length(store: &FakeStore, name: &str) -> Option<u64> {
    let raw = store
        .get_pref(crate::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY)
        .unwrap()?;
    serde_json::from_str::<std::collections::HashMap<String, u64>>(&raw)
        .unwrap()
        .get(name)
        .copied()
}

#[tokio::test]
async fn a_file_an_earlier_parser_could_not_read_is_fetched_again() {
    // Review rounds 4 and 5 of #130: the app version stays the same across
    // builds, so an update that can read the file is told by the parser.
    // The file is fetched again, by an exact device too; no catch-up.
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    hooks
        .store
        .set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, &unreadable_by(0))
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert_eq!(*hooks.catch_ups.lock().unwrap(), 0);
    assert_eq!(applied_length(&hooks.store, "x.jsonl"), Some(0));
    assert!(hooks
        .store
        .get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_file_this_parser_could_not_read_stays_named() {
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let record = unreadable_by(sync_core::PARSER_FINGERPRINT);
    hooks
        .store
        .set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, &record)
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert_eq!(*hooks.catch_ups.lock().unwrap(), 0);
    assert_eq!(
        hooks
            .store
            .get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY)
            .unwrap()
            .as_deref(),
        Some(record.as_str()),
    );
}

#[tokio::test]
async fn a_record_of_unreadable_files_that_does_not_read_makes_the_device_catch_up() {
    let (_tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    hooks
        .store
        .set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, "not a record")
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert_eq!(*hooks.catch_ups.lock().unwrap(), 1);
}

/// A peer's file of `bytes`, named an hour ago.
async fn peer_file(adapter: &Arc<dyn SyncAdapter>, bytes: &[u8]) -> String {
    let name = LogFileName::new(
        Utc::now() - chrono::Duration::hours(1),
        DeviceId::from_string("dev-desktop".into()),
    );
    adapter
        .push_log(&LogFile {
            name: name.clone(),
            bytes: bytes.to_vec(),
        })
        .await
        .unwrap();
    name.to_filename()
}

#[tokio::test]
async fn a_line_this_build_does_not_know_is_named_and_not_fetched_again_and_again() {
    // Review round 5 of #130: recorded as length 0, it was fetched again in
    // every round, for as long as it was there.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let bytes = b"{\"from\":\"a newer build\"}\n";
    let name = peer_file(&adapter, bytes).await;

    orchestrator.sync_now().await.expect("round");

    assert_eq!(
        applied_length(&hooks.store, &name),
        Some(bytes.len() as u64)
    );
    let raw = hooks.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY).unwrap();
    assert_eq!(crate::UnreadableLogs::names(raw.as_deref()), vec![name]);
}

#[tokio::test]
async fn a_torn_read_is_fetched_again_and_not_named() {
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let whole = String::from_utf8(session("dev-desktop", 1_000, 1).bytes).unwrap();
    let name = peer_file(&adapter, format!("{whole}\n{{\"id\":\"ev").as_bytes()).await;

    orchestrator.sync_now().await.expect("round");

    assert_eq!(applied_length(&hooks.store, &name), Some(0));
    assert!(hooks
        .store
        .get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY)
        .unwrap()
        .is_none());
}

// ---- Review round 5 of #130: a file pushed once does not go up again --------

/// An own pending file of envelopes at `secs`, named at the first, each line
/// ending with its newline.
fn own_file(secs: &[i64]) -> LogFile {
    let device_id = DeviceId::from_string("dev-laptop".into());
    let bytes: String = secs
        .iter()
        .map(|at| {
            let line = serde_json::to_string(&EventEnvelope {
                id: format!("ev-own-{at}"),
                device_id: device_id.clone(),
                timestamp: Utc.timestamp_opt(*at, 0).unwrap(),
                event: SyncEvent::EventDeleted(IdPayload {
                    id: format!("row-{at}"),
                }),
            })
            .unwrap();
            format!("{line}\n")
        })
        .collect();
    LogFile {
        name: LogFileName::new(Utc.timestamp_opt(secs[0], 0).unwrap(), device_id),
        bytes: bytes.into_bytes(),
    }
}

fn stage(tmp: &TempDir, log: &LogFile) {
    std::fs::write(
        tmp.path().join("pending").join(log.name.to_filename()),
        &log.bytes,
    )
    .unwrap();
}

fn pushed_record(store: &FakeStore, record: &[(&LogFile, usize)]) {
    let map: std::collections::HashMap<String, u64> = record
        .iter()
        .map(|(log, len)| (log.name.to_filename(), *len as u64))
        .collect();
    store
        .set_pref(
            crate::SYNC_PUSHED_LENGTHS_PREF_KEY,
            &crate::pushed::encode(&map),
        )
        .unwrap();
}

/// The remote's log files, by name, and what they hold.
async fn remote_logs(adapter: &Arc<dyn SyncAdapter>) -> Vec<(String, String)> {
    let mut logs: Vec<(String, String)> = adapter
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
        .unwrap()
        .into_iter()
        .map(|log| {
            (
                log.name.to_filename(),
                String::from_utf8(log.bytes).unwrap(),
            )
        })
        .collect();
    logs.sort();
    logs
}

fn pending_names(tmp: &TempDir) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(tmp.path().join("pending"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A dataset whose GC passed `horizon`; this device read up to well past it.
async fn gc_passed(
    horizon: i64,
) -> (
    TempDir,
    SyncOrchestrator,
    Arc<RecordingHooks>,
    Arc<dyn SyncAdapter>,
) {
    let mut meta = fresh_meta();
    meta.gc_horizon = Some(Utc.timestamp_opt(horizon, 0).unwrap());
    let (tmp, orchestrator, hooks, adapter) = device(meta, false).await;
    mark_exact(&hooks.store);
    hooks
        .store
        .set_pref(
            crate::SYNC_CURSOR_PREF_KEY,
            &Utc.timestamp_opt(horizon * 10, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    (tmp, orchestrator, hooks, adapter)
}

#[tokio::test]
async fn a_finished_session_pushed_whole_before_is_only_removed() {
    // Every app start pushed it once more, whole — after a GC, back to the
    // remote for a device that joined since to take as unknown. Here the GC
    // has not passed it: whether its copy is there or not, what was pushed
    // does not go again.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(500).await;
    let log = own_file(&[1_000, 1_001]);
    stage(&tmp, &log);
    pushed_record(&hooks.store, &[(&log, log.bytes.len())]);

    orchestrator.sync_now().await.expect("round");

    assert!(remote_logs(&adapter).await.is_empty());
    assert!(pending_names(&tmp).is_empty());
}

#[tokio::test]
async fn of_a_file_whose_remote_copy_may_be_gone_only_what_never_went_up_goes() {
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    let log = own_file(&[1_000, 2_000]);
    stage(&tmp, &log);
    let first_line = log.bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    pushed_record(&hooks.store, &[(&log, first_line)]);

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert_ne!(remote[0].0, log.name.to_filename());
    assert!(remote[0].1.contains("ev-own-2000"));
    assert!(!remote[0].1.contains("ev-own-1000"));
    assert!(pending_names(&tmp).is_empty());
}

#[tokio::test]
async fn the_first_push_of_this_build_goes_by_what_its_own_record_says_was_pushed() {
    // A 0.1.0 leftover: no record of what went up. This device's own entry
    // in meta.json says when its last round was; what came before it went.
    let mut meta = fresh_meta();
    meta.gc_horizon = Some(Utc.timestamp_opt(6_000, 0).unwrap());
    meta.upsert_device(
        &DeviceId::from_string("dev-laptop".into()),
        sync_core::DeviceRecord {
            name: None,
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            last_seen: Some(Utc.timestamp_opt(2_000 + 600, 0).unwrap()),
            app_version: "0.1.0".into(),
            stale: false,
            exact: false,
            unreadable: Vec::new(),
        },
    );
    let (tmp, orchestrator, hooks, adapter) = device(meta, false).await;
    mark_exact(&hooks.store);
    hooks
        .store
        .set_pref(
            crate::SYNC_CURSOR_PREF_KEY,
            &Utc.timestamp_opt(60_000, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    let log = own_file(&[1_000, 5_000]);
    stage(&tmp, &log);

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert!(remote[0].1.contains("ev-own-5000"));
    assert!(!remote[0].1.contains("ev-own-1000"));
    assert!(hooks
        .store
        .get_pref(crate::SYNC_PUSHED_LENGTHS_PREF_KEY)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn push_now_goes_by_the_datasets_gc_horizon_too() {
    // The phone pushes through push_now after each edit; without the meta it
    // took every finished file for one whose remote copy is there.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    let log = own_file(&[1_000, 2_000]);
    stage(&tmp, &log);
    let first_line = log.bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    pushed_record(&hooks.store, &[(&log, first_line)]);

    orchestrator.push_now().await.expect("push");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert!(!remote[0].1.contains("ev-own-1000"));
}

#[tokio::test]
async fn push_now_pushes_nothing_from_a_plaintext_device_into_an_encrypted_dataset() {
    let mut meta = fresh_meta();
    meta.e2e_enabled = true;
    let (tmp, orchestrator, _hooks, adapter) = device(meta, false).await;
    stage(&tmp, &own_file(&[1_000]));

    let result = orchestrator.push_now().await;

    assert!(
        matches!(result, Err(SyncError::EncryptionRequired)),
        "{result:?}"
    );
    assert!(remote_logs(&adapter).await.is_empty());
}

#[tokio::test]
async fn the_writer_leaves_a_live_file_the_gc_horizon_passed() {
    // A desktop runs for weeks; a peer's compaction deletes its live file,
    // unchanged long enough. Its next edit pushed the whole file again.
    let tmp = TempDir::new().expect("tempdir");
    let remote = tmp.path().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let adapter: Arc<dyn SyncAdapter> = Arc::new(LocalFsSyncAdapter::new(&remote));
    adapter.push_meta(&fresh_meta()).await.unwrap();
    let device_id = DeviceId::from_string("dev-laptop".into());
    let boot = Utc::now().with_nanosecond(0).unwrap() - chrono::Duration::hours(1);
    let writer = crate::EventLogWriter::spawn_with_kick(
        tmp.path().to_path_buf(),
        device_id.clone(),
        None,
        boot,
    );
    let pending = tmp.path().join("sync").join("log").join("pending");
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
        Some(Arc::clone(&writer)),
        None,
    ));
    let hooks = Arc::new(RecordingHooks {
        store: fake.clone(),
        catch_ups: Mutex::new(0),
        fail: false,
        horizon_meanwhile: Mutex::new(None),
    });
    let orchestrator = SyncOrchestrator::new(
        store,
        pending.clone(),
        device_id,
        applier,
        hooks.clone(),
        compactor,
        boot,
    );
    orchestrator.configure(Arc::clone(&adapter));
    mark_exact(&fake);
    let live = pending
        .join(LogFileName::new(boot, DeviceId::from_string("dev-laptop".into())).to_filename());
    let written = |n: usize| {
        let live = live.clone();
        async move {
            for _ in 0..200 {
                let lines = std::fs::read_to_string(&live).map_or(0, |text| text.lines().count());
                if lines >= n {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("the writer did not write {n} line(s)");
        }
    };
    writer.append(SyncEvent::EventDeleted(IdPayload { id: "row-1".into() }));
    written(1).await;
    orchestrator.push_now().await.expect("first push");
    let first = remote_logs(&adapter).await;
    assert_eq!(first.len(), 1);

    let mut meta = adapter.fetch_meta().await.unwrap().unwrap();
    meta.gc_horizon = Some(boot + chrono::Duration::minutes(30));
    adapter.push_meta(&meta).await.unwrap();
    writer.append(SyncEvent::EventDeleted(IdPayload { id: "row-2".into() }));
    written(2).await;
    orchestrator.push_now().await.expect("second push");

    assert!(writer.session_at() > boot, "rotated away");
    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 2, "{remote:?}");
    let old = remote.iter().find(|(name, _)| *name == first[0].0).unwrap();
    assert_eq!(old.1, first[0].1, "the old file did not go up again");
    let tail = remote.iter().find(|(name, _)| *name != first[0].0).unwrap();
    assert!(tail.1.contains("row-2") && !tail.1.contains("row-1"));
}

#[tokio::test]
async fn the_push_goes_by_a_gc_horizon_raised_while_the_device_caught_up() {
    // The catch-up fetches every file and can take minutes; a compaction
    // elsewhere raised the horizon meanwhile, and the push judged by the
    // copy read before.
    let (tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    hooks
        .store
        .set_pref(
            crate::SYNC_CURSOR_PREF_KEY,
            &Utc.timestamp_opt(50_000, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    *hooks.horizon_meanwhile.lock().unwrap() = Some(Utc.timestamp_opt(5_000, 0).unwrap());
    let log = own_file(&[1_000, 2_000]);
    stage(&tmp, &log);
    let first_line = log.bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    pushed_record(&hooks.store, &[(&log, first_line)]);

    orchestrator.sync_now().await.expect("round");

    assert_eq!(*hooks.catch_ups.lock().unwrap(), 1);
    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert!(!remote[0].1.contains("ev-own-1000"));
}
