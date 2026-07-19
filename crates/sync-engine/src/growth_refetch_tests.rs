//! Engine-level growth-refetch round-trip tests (the append-miss guard).
//!
//! The adapter crates each unit-test their own `wants_sized` listing
//! filter, and the orchestrator unit-tests `merge_applied_log_lengths` —
//! but nothing exercised the LOOP those parts form: record each applied
//! file's byte length after a round, persist the map, seed the next
//! round's cursor with it, and have the adapter's sized listing re-fetch
//! a peer's live session file that grew in between. A regression
//! anywhere in that shared plumbing (a pref-key rename, a cap bug in the
//! merge, a length-domain drift in the E2E wrapper) would pass every
//! existing test while silently reintroducing the append-miss data-loss
//! class for ALL adapters. These tests drive TWO full orchestrator
//! stacks over one shared on-disk remote to pin the whole loop — once in
//! plaintext, once through the [`EncryptingAdapter`], whose
//! plaintext→ciphertext known-length translation is exactly the kind of
//! silent drift this file exists to catch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use sync_adapter_local::LocalFsSyncAdapter;
use sync_core::{
    DeviceId, EncryptingAdapter, EventEnvelope, IdPayload, LogFile, LogFileName, MetaJson,
    SyncAdapter, SyncEvent, SyncResult, KEY_LEN,
};
use tempfile::TempDir;

use crate::test_support::{FakeSecrets, FakeStore};
use crate::{
    Compactor, EventLogApplier, SnapshotBuilder, SyncOrchestrator, SyncRoundHooks, SyncStore,
};

/// Hooks that satisfy the round without any meta.json bookkeeping. The
/// heartbeat is a platform concern (desktop/mobile wrap their onboarding
/// services); the growth loop under test never needs meta to exist.
struct NoopHooks;

#[async_trait::async_trait]
impl SyncRoundHooks for NoopHooks {
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
}

/// The (fixed) session-file timestamp. It never changes while the peer
/// keeps appending — that invariance is what makes the append-miss
/// possible in the first place: the plain timestamp cursor can never
/// re-surface the file once it has been seen.
fn session_ts() -> DateTime<Utc> {
    Utc.timestamp_opt(1_000, 0).unwrap()
}

/// One device's full engine stack: orchestrator + applier over its own
/// in-memory SQLite, its own staging (pending) dir and prefs store.
/// Only the remote — the shared tempdir — is common between devices.
struct DeviceStack {
    orchestrator: SyncOrchestrator,
    device_id: DeviceId,
    pending_dir: PathBuf,
}

fn build_stack(local_root: &Path, device: &str, adapter: Arc<dyn SyncAdapter>) -> DeviceStack {
    let device_id = DeviceId::from_string(device.into());
    let pending_dir = local_root.join(device).join("pending");
    std::fs::create_dir_all(&pending_dir).expect("create pending dir");
    let store: Arc<dyn SyncStore> = Arc::new(FakeStore::default());
    let applier = Arc::new(EventLogApplier::new(
        Arc::clone(&store),
        Arc::new(FakeSecrets::default()),
        Arc::new(cal_adapter_local::LocalAdapter::new(
            cal_adapter_local::test_support::open_test_db(),
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
    let orchestrator = SyncOrchestrator::new(
        store,
        pending_dir.clone(),
        device_id.clone(),
        applier,
        Arc::new(NoopHooks),
        compactor,
        // `boot_at` only guards EMPTY pending stubs against deletion; the
        // session files these tests stage are always non-empty, so any
        // instant at/below the session timestamp works.
        session_ts(),
    );
    orchestrator.configure(adapter);
    DeviceStack {
        orchestrator,
        device_id,
        pending_dir,
    }
}

fn envelope(device: &DeviceId, n: usize) -> EventEnvelope {
    EventEnvelope {
        id: format!("evt_{n:04}"),
        device_id: device.clone(),
        timestamp: Utc.timestamp_opt(1_000 + n as i64, 0).unwrap(),
        // A delete is the simplest envelope that flows through the whole
        // apply pipeline (a point DELETE is a no-op on rows that never
        // existed) — these tests pin the fetch/dedupe loop, not row state.
        event: SyncEvent::EventDeleted(IdPayload {
            id: format!("row-{n}"),
        }),
    }
}

/// (Over)write the device's live session file in its pending dir —
/// shaped exactly like the `EventLogWriter`'s output: the same filename
/// for the whole session, strictly growing JSONL content (re-serialising
/// a superset of the same envelopes keeps the earlier bytes a prefix).
/// Returns the plaintext byte length written.
fn write_session(stack: &DeviceStack, envelopes: &[EventEnvelope]) -> u64 {
    let log = LogFile::from_envelopes(stack.device_id.clone(), session_ts(), envelopes)
        .expect("serialise session log");
    std::fs::write(stack.pending_dir.join(log.name.to_filename()), &log.bytes)
        .expect("write session file");
    log.bytes.len() as u64
}

/// The append-miss loop both variants share: A pushes a two-envelope
/// session, B applies it; A appends three more envelopes to the SAME
/// filename; B's next round must re-fetch the file — its timestamp sits
/// AT B's cursor, so only the persisted applied-length record can
/// trigger the fetch — and apply EXACTLY the appended three, deduping
/// the already-applied prefix via the idempotency table. A further
/// round with no growth must fetch nothing. Returns the session
/// filename + its final plaintext length so callers can additionally
/// pin the remote's on-disk byte domain.
async fn assert_append_roundtrip(a: &DeviceStack, b: &DeviceStack) -> (String, u64) {
    let initial: Vec<EventEnvelope> = (0..2).map(|n| envelope(&a.device_id, n)).collect();
    write_session(a, &initial);

    let round = a.orchestrator.sync_now().await.expect("A round 1");
    assert_eq!(round.pushed_logs, 1, "A pushes its session file");

    let round = b.orchestrator.sync_now().await.expect("B round 1");
    assert_eq!(round.fetched_logs, 1, "B fetches A's session file");
    assert_eq!(round.applied, 2, "B applies the initial envelopes");
    assert_eq!(round.apply_failures, 0);

    // A's session keeps growing: three more envelopes, SAME filename.
    let grown: Vec<EventEnvelope> = (0..5).map(|n| envelope(&a.device_id, n)).collect();
    let plain_len = write_session(a, &grown);
    let round = a.orchestrator.sync_now().await.expect("A round 2");
    assert_eq!(round.pushed_logs, 1, "grown session file re-pushed");

    let round = b.orchestrator.sync_now().await.expect("B round 2");
    assert_eq!(
        round.fetched_logs, 1,
        "grown file re-fetched via known_lengths (its timestamp sits at the cursor)",
    );
    assert_eq!(round.applied, 3, "exactly the appended envelopes applied");
    assert_eq!(
        round.skipped_already_applied, 2,
        "the already-applied prefix deduped, not re-applied",
    );
    assert_eq!(round.apply_failures, 0);

    // No growth since → the recorded length matches the listing again.
    let round = b.orchestrator.sync_now().await.expect("B round 3");
    assert_eq!(round.fetched_logs, 0, "unchanged file not fetched again");
    assert_eq!(round.applied, 0);

    (
        LogFileName::new(session_ts(), a.device_id.clone()).to_filename(),
        plain_len,
    )
}

#[tokio::test]
async fn appended_envelopes_reach_the_peer_across_full_rounds() {
    let tmp = TempDir::new().expect("tempdir");
    let remote = tmp.path().join("remote");
    let a = build_stack(
        tmp.path(),
        "dev-a",
        Arc::new(LocalFsSyncAdapter::new(&remote)),
    );
    let b = build_stack(
        tmp.path(),
        "dev-b",
        Arc::new(LocalFsSyncAdapter::new(&remote)),
    );

    let (filename, plain_len) = assert_append_roundtrip(&a, &b).await;

    // Plaintext dataset: the remote file IS the raw JSONL, so the fs
    // listing size the growth check compares against equals the length
    // the engine recorded — the two domains coincide.
    let raw = std::fs::read(remote.join("log").join(&filename)).expect("remote log file");
    assert_eq!(
        raw.len() as u64,
        plain_len,
        "a plaintext remote log stores the raw JSONL bytes",
    );
}

#[tokio::test]
async fn appended_envelopes_reach_the_peer_through_the_encrypting_adapter() {
    // Same loop through the E2E wrapper (one shared dataset key, as after
    // §19.7 onboarding). This pins the known-length DOMAIN TRANSLATION:
    // the engine records PLAINTEXT byte lengths (it only ever sees
    // decrypted LogFiles), while the local-FS listing reports CIPHERTEXT
    // sizes — the EncryptingAdapter must bridge the two before the inner
    // adapter compares, or the growth check either re-downloads every
    // remembered file every round ("plaintext < ciphertext" always looks
    // grown — round 3 below catches that) or misses real growth (round 2
    // catches that).
    let tmp = TempDir::new().expect("tempdir");
    let remote = tmp.path().join("remote");
    let key = [7u8; KEY_LEN];
    let wrap = |root: &Path| -> Arc<dyn SyncAdapter> {
        Arc::new(EncryptingAdapter::new(
            Arc::new(LocalFsSyncAdapter::new(root)),
            key,
        ))
    };
    let a = build_stack(tmp.path(), "dev-a", wrap(&remote));
    let b = build_stack(tmp.path(), "dev-b", wrap(&remote));

    let (filename, plain_len) = assert_append_roundtrip(&a, &b).await;

    // Pin the ciphertext domain itself: the remote file must be exactly
    // plaintext + 28 (12-byte nonce + 16-byte GCM tag; GCM pads nothing).
    // If this overhead ever drifts, the wrapper's cursor translation and
    // the real remote sizes drift apart with it — and the round 2/3
    // assertions above start failing in whichever direction it drifted.
    let ciphertext = std::fs::read(remote.join("log").join(&filename)).expect("remote log file");
    assert_eq!(
        ciphertext.len() as u64,
        plain_len + 28,
        "encrypted remote log = plaintext + nonce(12) + GCM tag(16)",
    );
}
