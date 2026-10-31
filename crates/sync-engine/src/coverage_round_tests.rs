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
    SyncStore, CATCH_UP_RETRY, SYNC_CATCH_UP_TRIED_AT_PREF_KEY, SYNC_COVERAGE_EXACT_SINCE_PREF_KEY,
    SYNC_UNREADABLE_LOGS_PREF_KEY,
};

/// Counts catch-ups; a successful one marks the record exact, as the
/// platform's onboarding service does.
struct RecordingHooks {
    store: Arc<FakeStore>,
    catch_ups: Mutex<usize>,
    fail: bool,
    /// How many log files the remote held at each heartbeat.
    heartbeats: Mutex<Vec<usize>>,
    /// Heartbeats and catch-ups, in the order they ran.
    order: Mutex<Vec<&'static str>>,
}

#[async_trait::async_trait]
impl SyncRoundHooks for RecordingHooks {
    fn app_version(&self) -> String {
        "1.0.0-test".into()
    }

    async fn heartbeat(
        &self,
        adapter: &dyn SyncAdapter,
        _last_seen_log: DateTime<Utc>,
        _round_meta: Option<&MetaJson>,
    ) -> SyncResult<()> {
        let held = adapter
            .fetch_new_logs(&sync_core::DeviceCursor::epoch())
            .await?
            .len();
        self.heartbeats.lock().unwrap().push(held);
        self.order.lock().unwrap().push("heartbeat");
        Ok(())
    }

    async fn catch_up(&self, _adapter: &dyn SyncAdapter) -> SyncResult<ApplyReport> {
        *self.catch_ups.lock().unwrap() += 1;
        self.order.lock().unwrap().push("catch_up");
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

/// This device.
fn laptop() -> DeviceId {
    DeviceId::from_string("dev-laptop".into())
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
    device_booted(meta, fail, Utc::now()).await
}

/// [`device`], its app started at `boot_at`.
async fn device_booted(
    meta: MetaJson,
    fail: bool,
    boot_at: DateTime<Utc>,
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
        heartbeats: Mutex::new(Vec::new()),
        order: Mutex::new(Vec::new()),
    });
    let orchestrator = SyncOrchestrator::new(
        store,
        pending,
        device_id,
        applier,
        hooks.clone(),
        compactor,
        boot_at,
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
    hooks
        .store
        .set_pref(crate::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY, r#"{"x.jsonl":1}"#)
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert_eq!(*hooks.catch_ups.lock().unwrap(), 0);
    // Taken as unknown, also from an adapter that lists no sizes.
    assert_eq!(applied_length(&hooks.store, "x.jsonl"), None);
    // Still named until a round reads it (review round 6): a fetch failing
    // meanwhile must not lift the GC's guard on it.
    let raw = hooks.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY).unwrap();
    assert_eq!(
        crate::UnreadableLogs::names(raw.as_deref(), &laptop()),
        vec!["x.jsonl"]
    );
}

#[tokio::test]
async fn a_named_file_a_round_reads_is_named_no_more() {
    // Review round 6 of #130: named once, it held the GC back for as long as
    // the parser stayed the same, though this device had read it since.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let whole = session("dev-desktop", 1_000, 1);
    let name = peer_file(&adapter, &whole.bytes).await;
    let record = serde_json::to_string(&crate::UnreadableLogs {
        parser: sync_core::PARSER_FINGERPRINT,
        files: std::collections::HashMap::from([(name.clone(), 3)]),
    })
    .unwrap();
    hooks
        .store
        .set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, &record)
        .unwrap();

    orchestrator.sync_now().await.expect("round");

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
async fn a_named_file_torn_again_at_its_size_is_not_fetched_every_round() {
    // Review round 6 of #130: named for being torn twice at the same size,
    // it was fetched again in every ordinary round.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let whole = String::from_utf8(session("dev-desktop", 1_000, 1).bytes).unwrap();
    let torn = format!("{whole}\n{{\"id\":\"ev");
    let name = peer_file(&adapter, torn.as_bytes()).await;
    let record = serde_json::to_string(&crate::UnreadableLogs {
        parser: sync_core::PARSER_FINGERPRINT,
        files: std::collections::HashMap::from([(name.clone(), torn.len() as u64)]),
    })
    .unwrap();
    hooks
        .store
        .set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, &record)
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert_eq!(applied_length(&hooks.store, &name), Some(torn.len() as u64));
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
    assert_eq!(
        crate::UnreadableLogs::names(raw.as_deref(), &laptop()),
        vec![name]
    );
}

#[tokio::test]
async fn a_torn_read_is_fetched_again_and_left_once_torn_again_at_its_size() {
    // Review round 7 of #130: only a catch-up named a torn file, which an
    // exact device never runs, so it fetched a file torn for good in every
    // round.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let whole = String::from_utf8(session("dev-desktop", 1_000, 1).bytes).unwrap();
    let torn = format!("{whole}\n{{\"id\":\"ev");
    let name = peer_file(&adapter, torn.as_bytes()).await;

    orchestrator.sync_now().await.expect("round 1");
    assert_eq!(
        applied_length(&hooks.store, &name),
        Some(0),
        "fetched again"
    );

    // Review round 8 of #130: not in the record a catch-up trusts, which
    // would take it as no read in progress minutes later.
    assert!(hooks
        .store
        .get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY)
        .unwrap()
        .is_none());

    orchestrator.sync_now().await.expect("round 2");
    assert_eq!(
        applied_length(&hooks.store, &name),
        Some(torn.len() as u64),
        "torn again at the same size: no read in progress"
    );
}

// ---- Review rounds 5 and 6 of #130: what goes up, once ----------------------

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
    gc_passed_booted(horizon, Utc::now()).await
}

/// [`gc_passed`], the app started at `boot_at`.
async fn gc_passed_booted(
    horizon: i64,
    boot_at: DateTime<Utc>,
) -> (
    TempDir,
    SyncOrchestrator,
    Arc<RecordingHooks>,
    Arc<dyn SyncAdapter>,
) {
    let mut meta = fresh_meta();
    meta.gc_horizon = Some(Utc.timestamp_opt(horizon, 0).unwrap());
    let (tmp, orchestrator, hooks, adapter) = device_booted(meta, false, boot_at).await;
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
async fn a_finished_pending_file_a_full_disk_damaged_goes_up_as_its_whole_lines() {
    // Review round 14 of #130: a half line in the middle kept the file from
    // ever going up, and the changes after it never reached a peer.
    let (tmp, orchestrator, hooks, adapter) = device(fresh_meta(), true).await;
    mark_exact(&hooks.store);
    let whole = own_file(&[2_000, 2_001]);
    let text = String::from_utf8(whole.bytes.clone()).unwrap();
    let (first, second) = text.split_once('\n').unwrap();
    let damaged = LogFile {
        name: whole.name.clone(),
        bytes: format!("{first}\n{{\"id\":\"ev-half\n{second}").into_bytes(),
    };
    stage(&tmp, &damaged);

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert!(remote[0].1.contains("ev-own-2000"));
    assert!(remote[0].1.contains("ev-own-2001"));
    assert!(!remote[0].1.contains("ev-half"));
    assert!(pending_names(&tmp).is_empty(), "pushed whole, it is gone");
}

#[tokio::test]
async fn a_directory_named_as_a_leftover_holds_no_settle_back() {
    // Review round 14 of #130: a directory's size is not 0 everywhere; named
    // as a leftover it was never read, and every leftover stayed held. (On
    // Windows its size is 0: this test bites on Linux and macOS.)
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    stage(&tmp, &own_file(&[1_000, 4_000]));
    let dir = LogFileName::new(
        Utc.timestamp_opt(1_500, 0).unwrap(),
        DeviceId::from_string("dev-laptop".into()),
    );
    std::fs::create_dir(tmp.path().join("pending").join(dir.to_filename())).unwrap();
    std::fs::write(
        tmp.path()
            .join("pending")
            .join(dir.to_filename())
            .join("inside"),
        b"x",
    )
    .unwrap();
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert!(hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_SETTLED_PREF_KEY)
        .unwrap()
        .is_some());
    assert!(remote_logs(&adapter)
        .await
        .iter()
        .any(|(_, text)| text.contains("ev-own-4000")));
}

#[tokio::test]
async fn no_round_runs_while_rounds_are_held() {
    // Review round 14 of #130: the manual resume ran beside a round, and its
    // later snapshot apply reverted what the round had applied.
    let (_tmp, orchestrator, _hooks, _adapter) = device(fresh_meta(), true).await;
    let hold = orchestrator.hold_rounds().await.expect("held");

    assert!(orchestrator.sync_now().await.is_err());
    drop(hold);
    orchestrator.sync_now().await.expect("a round once let go");
}

#[tokio::test]
async fn holding_rounds_waits_for_the_running_one() {
    let (_tmp, orchestrator, _hooks, _adapter) = device(fresh_meta(), true).await;
    let running = orchestrator.hold_rounds().await.expect("held");

    let (second, ()) = tokio::join!(orchestrator.hold_rounds(), async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        drop(running);
    });

    assert!(second.is_ok());
}

#[tokio::test]
async fn the_pending_files_are_named_before_any_goes_up() {
    // Review round 6 of #130: no compactor deletes a file a device names as
    // pending, so the names must be in meta.json before the file is on the
    // remote.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(500).await;
    stage(&tmp, &own_file(&[1_000]));

    orchestrator.sync_now().await.expect("round");

    assert_eq!(hooks.heartbeats.lock().unwrap().first(), Some(&0));
    assert_eq!(remote_logs(&adapter).await.len(), 1);
}

#[tokio::test]
async fn an_older_builds_leftover_sends_only_what_came_after_its_last_round() {
    // Review rounds 5 and 6 of #130: a 0.1.0 build kept every pushed file in
    // pending/, and its compactors deleted by name; pushed whole once more,
    // a leftover brought months-old changes back for devices that never
    // applied them.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert_ne!(remote[0].0, log.name.to_filename());
    assert!(remote[0].1.contains("ev-own-4000"));
    assert!(!remote[0].1.contains("ev-own-1000"));
    assert!(pending_names(&tmp)
        .iter()
        .all(|name| *name != log.name.to_filename()));
    assert!(hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_SETTLED_PREF_KEY)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_leftover_with_no_round_known_goes_up_whole() {
    let (tmp, orchestrator, _hooks, adapter) = gc_passed(5_000).await;
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert_eq!(remote[0].0, log.name.to_filename());
}

#[tokio::test]
async fn the_leftovers_are_settled_once() {
    // A file of this build, finished and still pending (its push failed),
    // named below the horizon later, is this build's work: whole.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    hooks
        .store
        .set_pref(crate::SYNC_LEFTOVERS_SETTLED_PREF_KEY, "1")
        .unwrap();
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert_eq!(remote[0].0, log.name.to_filename());
}

#[tokio::test]
async fn another_remote_gets_every_pending_file_once_more() {
    // Review round 6 of #130: an adapter swap, an E2E switch, a join — the
    // per-session lengths say nothing about the new remote.
    let (tmp, orchestrator, _hooks, _adapter) = device(fresh_meta(), false).await;
    let live = own_file(&[Utc::now().timestamp() + 3_600]);
    stage(&tmp, &live);
    orchestrator.push_now().await.expect("first push");
    let other_dir = tmp.path().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other: Arc<dyn SyncAdapter> = Arc::new(LocalFsSyncAdapter::new(&other_dir));
    other.push_meta(&fresh_meta()).await.unwrap();

    orchestrator.configure(Arc::clone(&other));
    orchestrator.push_now().await.expect("push to the other");

    assert_eq!(remote_logs(&other).await.len(), 1);
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

// ---- Review round 7 of #130 ------------------------------------------------

#[tokio::test]
async fn the_pending_files_are_named_before_the_round_catches_up() {
    // A catch-up takes minutes; a compactor deleting meanwhile deleted what
    // this device then pushed again, named only after it.
    let (tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    stage(&tmp, &own_file(&[1_000]));

    orchestrator.sync_now().await.expect("round");

    let order = hooks.order.lock().unwrap().clone();
    assert_eq!(order.first(), Some(&"heartbeat"), "{order:?}");
    assert!(order.contains(&"catch_up"), "{order:?}");
}

#[tokio::test]
async fn an_older_format_is_locked_out_before_the_round_catches_up() {
    // Nothing pending to name: the heartbeat still comes first, for the lock-out.
    let mut meta = fresh_meta();
    meta.schema_version = 1;
    let (_tmp, orchestrator, hooks, _adapter) = device(meta, false).await;

    orchestrator.sync_now().await.expect("round");

    let order = hooks.order.lock().unwrap().clone();
    assert_eq!(order.first(), Some(&"heartbeat"), "{order:?}");
}

#[tokio::test]
async fn an_empty_pending_file_is_not_named() {
    // It never goes up, yet named it cost a meta.json write at every start.
    let (tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    stage(
        &tmp,
        &LogFile {
            name: LogFileName::new(Utc::now() + chrono::Duration::hours(1), laptop()),
            bytes: Vec::new(),
        },
    );

    orchestrator.sync_now().await.expect("round");

    assert_eq!(
        hooks.order.lock().unwrap().clone(),
        vec!["heartbeat"],
        "only the round's own"
    );
}

#[tokio::test]
async fn a_leftover_still_on_the_remote_goes_up_whole() {
    // An older build's push after its last round may have torn the remote
    // copy; tail-only, nothing made it whole again.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);
    adapter
        .push_log(&LogFile {
            name: log.name.clone(),
            bytes: log.bytes[..20].to_vec(),
        })
        .await
        .unwrap();
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert_eq!(
        remote_logs(&adapter).await,
        vec![(
            log.name.to_filename(),
            String::from_utf8(log.bytes.clone()).unwrap()
        )]
    );
}

/// Lists no log names; everything else as the local-FS remote does.
struct ListlessAdapter {
    inner: LocalFsSyncAdapter,
    /// How often it was asked to list.
    lists: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl SyncAdapter for ListlessAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        self.inner.test_connection().await
    }
    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        self.inner.fetch_meta().await
    }
    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        self.inner.push_meta(meta).await
    }
    async fn fetch_new_logs(&self, since: &sync_core::DeviceCursor) -> SyncResult<Vec<LogFile>> {
        self.inner.fetch_new_logs(since).await
    }
    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        self.inner.push_log(log).await
    }
    async fn fetch_snapshot(&self) -> SyncResult<Option<sync_core::Snapshot>> {
        self.inner.fetch_snapshot().await
    }
    async fn push_snapshot(&self, snapshot: &sync_core::Snapshot) -> SyncResult<()> {
        self.inner.push_snapshot(snapshot).await
    }
    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        self.inner.delete_log(name).await
    }
    async fn list_log_names(&self) -> SyncResult<Vec<LogFileName>> {
        *self.lists.lock().unwrap() += 1;
        Err(SyncError::network("simulated listing outage"))
    }
    async fn push_sound_asset(&self, hash: &str, ext: &str, bytes: &[u8]) -> SyncResult<()> {
        self.inner.push_sound_asset(hash, ext, bytes).await
    }
    async fn fetch_sound_asset(&self, hash: &str, ext: &str) -> SyncResult<Option<Vec<u8>>> {
        self.inner.fetch_sound_asset(hash, ext).await
    }
}

#[tokio::test]
async fn a_leftover_not_settled_yet_stays_and_is_settled_by_its_first_round_later() {
    // A leftover settle could not decide on went up whole all the same; and
    // the next try cut it by a round time this build had moved meanwhile, so
    // what never went up was dropped.
    let (tmp, orchestrator, hooks, adapter) = gc_passed(5_000).await;
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    let lists = Arc::new(Mutex::new(0));
    orchestrator.configure(Arc::new(ListlessAdapter {
        inner: LocalFsSyncAdapter::new(tmp.path().join("remote")),
        lists: Arc::clone(&lists),
    }));

    orchestrator.sync_now().await.expect("round 1");
    assert!(remote_logs(&adapter).await.is_empty(), "held back");
    assert!(pending_names(&tmp).contains(&log.name.to_filename()));
    // Review round 8 of #130: on most adapters the listing fetches every
    // file; it was tried again in every round and push.
    orchestrator.sync_now().await.expect("round 2");
    assert_eq!(*lists.lock().unwrap(), 1, "not again right away");
    assert!(remote_logs(&adapter).await.is_empty(), "still held back");

    let earlier = Utc::now() - CATCH_UP_RETRY - chrono::Duration::minutes(1);
    hooks
        .store
        .set_pref(
            crate::SYNC_LEFTOVERS_TRIED_AT_PREF_KEY,
            &earlier.to_rfc3339(),
        )
        .unwrap();
    orchestrator.configure(Arc::clone(&adapter));
    orchestrator.sync_now().await.expect("an hour later");
    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert_ne!(remote[0].0, log.name.to_filename());
    assert!(remote[0].1.contains("ev-own-4000"), "{remote:?}");
    assert!(!remote[0].1.contains("ev-own-1000"), "{remote:?}");
}

#[tokio::test]
async fn an_unreadable_file_is_read_again_only_once_its_length_is_forgotten() {
    // Stamped with the new parser while the old length stayed, the file was
    // never fetched again.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    hooks
        .store
        .set_pref(SYNC_UNREADABLE_LOGS_PREF_KEY, &unreadable_by(0))
        .unwrap();
    hooks
        .store
        .set_pref(crate::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY, r#"{"x.jsonl":1}"#)
        .unwrap();
    hooks
        .store
        .failing_pref_writes
        .lock()
        .unwrap()
        .insert(crate::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY.to_string());
    // Review round 8 of #130: a file the round adds to the record replaced
    // the one not stamped yet.
    let added = peer_file(&adapter, b"{\"from\":\"a newer build\"}\n").await;

    orchestrator.sync_now().await.expect("round 1");
    let raw = hooks.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY).unwrap();
    assert!(
        crate::UnreadableLogs::of_this_parser(raw.as_deref()).is_none(),
        "not stamped while the length stands"
    );
    assert_eq!(
        crate::UnreadableLogs::names(raw.as_deref(), &laptop()),
        {
            let mut names = vec![added, "x.jsonl".to_string()];
            names.sort();
            names
        },
        "still named, the round's addition too"
    );

    hooks.store.failing_pref_writes.lock().unwrap().clear();
    orchestrator.sync_now().await.expect("round 2");
    assert_eq!(applied_length(&hooks.store, "x.jsonl"), None);
    let raw = hooks.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY).unwrap();
    assert!(crate::UnreadableLogs::of_this_parser(raw.as_deref()).is_some());
}

#[test]
fn a_device_names_only_other_devices_files_as_unreadable() {
    let own = LogFileName::new(Utc.timestamp_opt(1_000, 0).unwrap(), laptop()).to_filename();
    let other = LogFileName::new(
        Utc.timestamp_opt(1_000, 0).unwrap(),
        DeviceId::from_string("dev-desktop".into()),
    )
    .to_filename();
    let raw = serde_json::to_string(&crate::UnreadableLogs {
        parser: sync_core::PARSER_FINGERPRINT,
        files: std::collections::HashMap::from([(own, 3), (other.clone(), 4)]),
    })
    .unwrap();

    assert_eq!(
        crate::UnreadableLogs::names(Some(&raw), &laptop()),
        vec![other]
    );
}

// ---- Review round 8 of #130 ------------------------------------------------

/// [`own_file`] staged, the device's last 0.1.0 round at 2 600 (cutoff
/// 2 000): only `ev-own-4000` never went up, and the remote lost the file.
async fn with_a_leftover(
    boot_at: DateTime<Utc>,
) -> (
    TempDir,
    SyncOrchestrator,
    Arc<RecordingHooks>,
    Arc<dyn SyncAdapter>,
    LogFile,
) {
    let (tmp, orchestrator, hooks, adapter) = gc_passed_booted(5_000, boot_at).await;
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    (tmp, orchestrator, hooks, adapter, log)
}

#[tokio::test]
async fn the_round_waits_while_the_leftovers_cutoff_cannot_be_frozen() {
    // A round recorded itself though the cutoff was not kept; the next try
    // read this build's round time and dropped what never went up.
    let (_tmp, orchestrator, hooks, adapter, _log) = with_a_leftover(Utc::now()).await;
    hooks
        .store
        .failing_pref_writes
        .lock()
        .unwrap()
        .insert(crate::SYNC_LEFTOVERS_CUTOFF_PREF_KEY.to_string());

    orchestrator.sync_now().await.expect("round 1");
    assert!(remote_logs(&adapter).await.is_empty(), "nothing settled");
    assert_eq!(
        hooks
            .store
            .get_pref(crate::SYNC_LAST_ROUND_PREF_KEY)
            .unwrap()
            .as_deref(),
        Some(Utc.timestamp_opt(2_600, 0).unwrap().to_rfc3339().as_str()),
        "the older build's round stands"
    );

    hooks.store.failing_pref_writes.lock().unwrap().clear();
    orchestrator.sync_now().await.expect("round 2");
    let remote = remote_logs(&adapter).await;
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert!(remote[0].1.contains("ev-own-4000"), "{remote:?}");
    assert!(!remote[0].1.contains("ev-own-1000"), "{remote:?}");
}

#[tokio::test]
async fn a_tail_takes_no_name_a_file_on_the_remote_has() {
    // A tail an earlier try pushed is gone from pending/: its name was taken
    // again, and the remote copy overwritten.
    let boot = Utc.timestamp_opt(Utc::now().timestamp(), 0).unwrap();
    let (_tmp, orchestrator, _hooks, adapter, log) = with_a_leftover(boot).await;
    let earlier_tail = LogFile {
        name: LogFileName::new(boot - chrono::Duration::seconds(1), laptop()),
        bytes: b"{\"pushed\":\"before\"}\n".to_vec(),
    };
    adapter.push_log(&earlier_tail).await.unwrap();

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert!(
        remote.contains(&(
            earlier_tail.name.to_filename(),
            "{\"pushed\":\"before\"}\n".to_string()
        )),
        "{remote:?}"
    );
    assert!(
        remote
            .iter()
            .any(|(name, bytes)| *name != log.name.to_filename() && bytes.contains("ev-own-4000")),
        "{remote:?}"
    );
}

/// Holds `path` open so that it does not read (Windows: shared with nobody;
/// elsewhere: no permission) until dropped.
struct Unreadable {
    #[cfg(windows)]
    _file: std::fs::File,
    #[cfg(not(windows))]
    path: std::path::PathBuf,
}

impl Unreadable {
    fn hold(path: std::path::PathBuf) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path)
                .unwrap();
            Self { _file: file }
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            Self { path }
        }
    }
}

#[cfg(not(windows))]
impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o644));
    }
}

#[tokio::test]
async fn a_leftover_that_does_not_read_is_neither_settled_nor_pushed() {
    // Skipped by the read, it was no leftover to settle: the mark was set,
    // and a later push sent it whole.
    let (tmp, orchestrator, hooks, adapter, log) = with_a_leftover(Utc::now()).await;
    let path = tmp.path().join("pending").join(log.name.to_filename());
    {
        let _held = Unreadable::hold(path);
        orchestrator.sync_now().await.expect("round 1");
    }
    assert!(hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_SETTLED_PREF_KEY)
        .unwrap()
        .is_none());

    orchestrator
        .push_now()
        .await
        .expect("push, the file reads again");
    let remote = remote_logs(&adapter).await;
    assert!(
        remote
            .iter()
            .all(|(name, _)| *name != log.name.to_filename()),
        "{remote:?}"
    );
    assert!(
        remote
            .iter()
            .any(|(_, bytes)| bytes.contains("ev-own-4000")),
        "settled now: {remote:?}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_leftover_that_will_not_go_is_emptied_so_no_later_try_cuts_it_again() {
    // Kept whole, every later try wrote and pushed its tail once more.
    use std::os::windows::fs::OpenOptionsExt;
    let (tmp, orchestrator, hooks, adapter, log) = with_a_leftover(Utc::now()).await;
    let path = tmp.path().join("pending").join(log.name.to_filename());
    {
        // Shared for reading and writing, not for deleting.
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&path)
            .unwrap();
        orchestrator.sync_now().await.expect("round 1");
    }
    orchestrator.sync_now().await.expect("round 2");

    assert!(hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_SETTLED_PREF_KEY)
        .unwrap()
        .is_some());
    let tails: Vec<_> = remote_logs(&adapter)
        .await
        .into_iter()
        .filter(|(_, bytes)| bytes.contains("ev-own-4000"))
        .collect();
    assert_eq!(tails.len(), 1, "{tails:?}");
}

// ---- Review round 9 of #130 ------------------------------------------------

#[tokio::test]
async fn a_file_this_build_cannot_read_and_could_not_name_is_fetched_again() {
    // Its length was recorded before the record failed to read: never fetched
    // again, never named, never read by a later parser.
    let (_tmp, orchestrator, hooks, adapter) = device(fresh_meta(), false).await;
    mark_exact(&hooks.store);
    let bytes = b"{\"from\":\"a newer build\"}\n";
    let name = peer_file(&adapter, bytes).await;
    hooks
        .store
        .failing_pref_reads
        .lock()
        .unwrap()
        .insert(SYNC_UNREADABLE_LOGS_PREF_KEY.to_string());

    orchestrator.sync_now().await.expect("round 1");
    // No length, not 0: a listing without sizes reads 0 as known (review
    // round 10 of #130).
    assert_eq!(applied_length(&hooks.store, &name), None);

    hooks.store.failing_pref_reads.lock().unwrap().clear();
    orchestrator.sync_now().await.expect("round 2");
    assert_eq!(
        applied_length(&hooks.store, &name),
        Some(bytes.len() as u64)
    );
    let raw = hooks.store.get_pref(SYNC_UNREADABLE_LOGS_PREF_KEY).unwrap();
    assert_eq!(
        crate::UnreadableLogs::names(raw.as_deref(), &laptop()),
        vec![name]
    );
}

#[tokio::test]
async fn while_the_leftovers_wait_this_builds_own_finished_files_go_up() {
    // Held by time, every finished file below the horizon waited, this
    // build's sessions too, for as long as settling did.
    let first_run = Utc::now() - chrono::Duration::hours(2);
    let (tmp, orchestrator, hooks, adapter) =
        gc_passed_booted(Utc::now().timestamp() + 3_600, Utc::now()).await;
    hooks
        .store
        .set_pref(
            crate::SYNC_LEFTOVERS_CUTOFF_PREF_KEY,
            &serde_json::to_string(&crate::LeftoversCutoff {
                last_round: Some(Utc.timestamp_opt(2_600, 0).unwrap()),
                before: first_run,
            })
            .unwrap(),
        )
        .unwrap();
    let leftover = own_file(&[1_000, 4_000]);
    stage(&tmp, &leftover);
    let earlier_session = own_file(&[(first_run + chrono::Duration::minutes(30)).timestamp()]);
    stage(&tmp, &earlier_session);
    orchestrator.configure(Arc::new(ListlessAdapter {
        inner: LocalFsSyncAdapter::new(tmp.path().join("remote")),
        lists: Arc::new(Mutex::new(0)),
    }));

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert!(
        remote
            .iter()
            .any(|(name, _)| *name == earlier_session.name.to_filename()),
        "{remote:?}"
    );
    assert!(
        remote
            .iter()
            .all(|(name, _)| *name != leftover.name.to_filename()),
        "the leftover waits: {remote:?}"
    );
}

#[tokio::test]
async fn a_round_without_meta_json_freezes_the_leftovers_cutoff_too() {
    // It recorded this build's time as the last successful round before
    // anything froze the older build's.
    let (tmp, orchestrator, hooks, _adapter) = device(fresh_meta(), false).await;
    std::fs::remove_file(tmp.path().join("remote").join("meta.json")).unwrap();
    let round = Utc.timestamp_opt(2_600, 0).unwrap();
    hooks
        .store
        .set_pref(crate::SYNC_LAST_ROUND_PREF_KEY, &round.to_rfc3339())
        .unwrap();

    let _ = orchestrator.sync_now().await;

    let raw = hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_CUTOFF_PREF_KEY)
        .unwrap()
        .expect("frozen");
    assert_eq!(
        crate::LeftoversCutoff::parse(&raw).unwrap().last_round,
        Some(round)
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_leftover_that_will_neither_go_nor_empty_sends_its_tail_once() {
    // Each try listed the remote again and pushed one more copy of its tail.
    use std::os::windows::fs::OpenOptionsExt;
    let (tmp, orchestrator, hooks, adapter, log) = with_a_leftover(Utc::now()).await;
    let path = tmp.path().join("pending").join(log.name.to_filename());
    // Shared for reading only: it can be neither removed nor emptied.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(&path)
        .unwrap();

    orchestrator.sync_now().await.expect("round 1");
    assert!(hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_TRIED_AT_PREF_KEY)
        .unwrap()
        .is_some());
    let earlier = Utc::now() - CATCH_UP_RETRY - chrono::Duration::minutes(1);
    hooks
        .store
        .set_pref(
            crate::SYNC_LEFTOVERS_TRIED_AT_PREF_KEY,
            &earlier.to_rfc3339(),
        )
        .unwrap();
    orchestrator.sync_now().await.expect("round 2");

    let tails: Vec<_> = remote_logs(&adapter)
        .await
        .into_iter()
        .filter(|(_, bytes)| bytes.contains("ev-own-4000"))
        .collect();
    assert_eq!(tails.len(), 1, "{tails:?}");
}

// ---- Review round 10 of #130 -----------------------------------------------

#[tokio::test]
async fn the_session_that_froze_the_cutoff_is_this_builds_own() {
    // Its file is named to the second, the cutoff to the instant: it counted
    // as an older build's and waited as long as settling did.
    let first_boot = Utc
        .timestamp_opt(Utc::now().timestamp() - 7_200, 734_000_000)
        .unwrap();
    let (tmp, orchestrator, hooks, adapter) =
        gc_passed_booted(Utc::now().timestamp() + 3_600, Utc::now()).await;
    hooks
        .store
        .set_pref(
            crate::SYNC_LEFTOVERS_CUTOFF_PREF_KEY,
            &serde_json::to_string(&crate::LeftoversCutoff {
                last_round: Some(Utc.timestamp_opt(2_600, 0).unwrap()),
                before: first_boot,
            })
            .unwrap(),
        )
        .unwrap();
    stage(&tmp, &own_file(&[1_000, 4_000]));
    let first_session = own_file(&[first_boot.timestamp()]);
    stage(&tmp, &first_session);
    orchestrator.configure(Arc::new(ListlessAdapter {
        inner: LocalFsSyncAdapter::new(tmp.path().join("remote")),
        lists: Arc::new(Mutex::new(0)),
    }));

    orchestrator.sync_now().await.expect("round");

    let remote = remote_logs(&adapter).await;
    assert!(
        remote
            .iter()
            .any(|(name, _)| *name == first_session.name.to_filename()),
        "{remote:?}"
    );
}

#[tokio::test]
async fn a_leftovers_cutoff_that_does_not_read_is_frozen_anew_with_no_round_known() {
    // Kept, it held every join and resume back for good. Frozen anew from
    // sync.lastSuccessfulRound, it read this build's round time, which counts
    // what never went up as pushed (review round 11 of #130).
    let (_tmp, orchestrator, hooks, _adapter, _log) = with_a_leftover(Utc::now()).await;
    hooks
        .store
        .set_pref(
            crate::SYNC_LEFTOVERS_CUTOFF_PREF_KEY,
            "2026-10-30T10:00:00Z",
        )
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    let raw = hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_CUTOFF_PREF_KEY)
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::LeftoversCutoff::parse(&raw).map(|cutoff| cutoff.last_round),
        Some(None)
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_tail_goes_up_while_another_leftover_waits() {
    // Named before the live file, it was held as an older build's file.
    use std::os::windows::fs::OpenOptionsExt;
    // The horizon past this session's start, so the hold reaches the tail.
    let (tmp, orchestrator, hooks, adapter) =
        gc_passed_booted(Utc::now().timestamp() + 3_600, Utc::now()).await;
    let log = own_file(&[1_000, 4_000]);
    stage(&tmp, &log);
    hooks
        .store
        .set_pref(
            crate::SYNC_LAST_ROUND_PREF_KEY,
            &Utc.timestamp_opt(2_000 + 600, 0).unwrap().to_rfc3339(),
        )
        .unwrap();
    let stuck = own_file(&[1_500, 4_500]);
    stage(&tmp, &stuck);
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(tmp.path().join("pending").join(stuck.name.to_filename()))
        .unwrap();

    orchestrator.sync_now().await.expect("round");

    assert!(hooks
        .store
        .get_pref(crate::SYNC_LEFTOVERS_SETTLED_PREF_KEY)
        .unwrap()
        .is_none());
    let remote = remote_logs(&adapter).await;
    assert!(
        remote
            .iter()
            .any(|(name, bytes)| *name != log.name.to_filename() && bytes.contains("ev-own-4000")),
        "{remote:?}"
    );
}
