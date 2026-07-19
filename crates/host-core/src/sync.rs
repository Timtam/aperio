//! The sync assembly graph — shared by the desktop backend and the mobile
//! UniFFI host.
//!
//! Both platforms stand up the SAME `sync-engine` components in the SAME order
//! (device id → writer → store → applier → snapshot builder → compactor →
//! onboarding → round hooks → orchestrator). [`build_orchestrator`] is that one
//! construction path; the desktop and the mobile host each call it with their
//! own injected [`SecretStore`] (OS keyring vs. the Keychain/Keystore bridge)
//! and drive the returned orchestrator differently (the desktop `SyncScheduler`
//! vs. the mobile JS-driven triggers).

use std::path::PathBuf;
use std::sync::Arc;

use cal_adapter_local::LocalAdapter;
use chrono::{DateTime, Utc};
use sync_core::DeviceId;
use sync_engine::{
    Compactor, EventLogApplier, EventLogWriter, SecretStore, SnapshotBuilder, SyncOrchestrator,
    SyncStore,
};
use tokio::sync::Notify;

use crate::db::SharedConn;
use crate::event_log::{
    load_or_mint_device_id, DesktopSyncRoundHooks, DesktopSyncStore, OnboardingService,
};
use crate::sound_assets::sounds_dir_under;

/// The assembled sync stack. The caller wires the parts into its own surface:
/// the desktop into Tauri State + the `SyncScheduler`; the mobile host into the
/// `Host` object + the JS-driven trigger model.
pub struct SyncGraph {
    /// Runs a sync round (push + fetch + apply + compaction audit).
    pub orchestrator: Arc<SyncOrchestrator>,
    /// Appends local mutations to the pending event log; the caller appends a
    /// `SyncEvent` after every local write.
    pub writer: Arc<EventLogWriter>,
    /// Snapshot consume/produce + `meta.json` heartbeats (preview/accept/adopt).
    pub onboarding: Arc<OnboardingService>,
    /// Rolls the session file + sweeps snapshotted pending files.
    pub compactor: Arc<Compactor>,
    /// This device's stable id (already moved into the orchestrator; a clone
    /// for the caller's logging / status surface).
    pub device_id: DeviceId,
    /// The writer pings this on every append; a scheduler awaits it to debounce
    /// a sync round. The desktop scheduler awaits it; the mobile host drives
    /// rounds from JS instead, but still holds it for parity.
    pub kick: Arc<Notify>,
}

/// Build the full sync graph against `db` + `data_dir`, reading/writing secrets
/// through the injected `secret_store`. `boot_at` MUST be a single instant
/// captured once by the caller and is threaded to BOTH the writer (which names
/// its session file with it) and the orchestrator (its stale-stub cleanup
/// guard) — see [`EventLogWriter::spawn_with_kick`].
///
/// MUST be called inside a Tokio runtime context: the writer starts its drain
/// task with `tokio::spawn`.
pub fn build_orchestrator(
    db: SharedConn,
    data_dir: PathBuf,
    secret_store: Arc<dyn SecretStore>,
    app_version: &str,
    boot_at: DateTime<Utc>,
) -> SyncGraph {
    let device_id = load_or_mint_device_id(&db);

    // The writer + a (future) scheduler share this Notify so a local mutation
    // can kick a debounced round.
    let kick = Arc::new(Notify::new());

    let writer = EventLogWriter::spawn_with_kick(
        data_dir.clone(),
        device_id.clone(),
        Some(Arc::clone(&kick)),
        boot_at,
    );

    // The applier/snapshot/compactor reach local storage through two seams: the
    // SQLite-backed SyncStore (via its own LocalAdapter on the same SharedConn)
    // and the injected SecretStore. One store, cloned across all three.
    // DELIBERATELY without the WAL read pool: the applier's reads are
    // write-adjacent (read-then-write inside apply flows), and a pooled
    // reader only sees COMMITTED state — it must not miss rows the apply
    // wrote moments (or a transaction) earlier on the writer connection.
    let applier_adapter = Arc::new(LocalAdapter::new(db.clone()));
    let sync_store: Arc<dyn SyncStore> = Arc::new(DesktopSyncStore::new(
        db.clone(),
        Arc::clone(&applier_adapter),
    ));
    let applier = Arc::new(EventLogApplier::new(
        Arc::clone(&sync_store),
        Arc::clone(&secret_store),
        Arc::clone(&applier_adapter),
        device_id.clone(),
    ));
    let snapshot_builder = Arc::new(SnapshotBuilder::new(
        Arc::clone(&sync_store),
        Arc::clone(&secret_store),
        app_version,
    ));

    let pending_dir = data_dir.join("sync").join("log").join("pending");
    let compactor = Arc::new(Compactor::new(
        Arc::clone(&sync_store),
        Arc::clone(&snapshot_builder),
        device_id.clone(),
        app_version,
        Some(Arc::clone(&writer)),
        Some(pending_dir.clone()),
    ));

    let sounds_dir = sounds_dir_under(&data_dir);
    let onboarding = Arc::new(OnboardingService::new(
        db.clone(),
        device_id.clone(),
        Arc::clone(&applier),
        Arc::clone(&snapshot_builder),
        pending_dir.clone(),
        sounds_dir.clone(),
        app_version,
    ));
    let round_hooks = Arc::new(DesktopSyncRoundHooks::new(
        db,
        Arc::clone(&onboarding),
        sounds_dir,
    ));
    let orchestrator = Arc::new(SyncOrchestrator::new(
        Arc::clone(&sync_store),
        pending_dir,
        device_id.clone(),
        applier,
        round_hooks,
        Arc::clone(&compactor),
        boot_at,
    ));

    SyncGraph {
        orchestrator,
        writer,
        onboarding,
        compactor,
        device_id,
        kick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use sync_engine::test_support::FakeSecrets;

    #[test]
    fn builds_an_unconfigured_graph() {
        let dir = tempfile::tempdir().unwrap();
        let db = DbHandle::open(dir.path().join("aperio.sqlite")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let secret_store: Arc<dyn SecretStore> = Arc::new(FakeSecrets::default());
        let graph = rt.block_on(async {
            build_orchestrator(
                db.shared(),
                dir.path().to_path_buf(),
                secret_store,
                "0.1.0-test",
                Utc::now(),
            )
        });
        // No adapter configured yet → status reports unconfigured + a sync_now
        // refuses cleanly rather than panicking.
        assert!(!graph.orchestrator.status().configured);
        let err = rt
            .block_on(async { graph.orchestrator.sync_now().await })
            .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("adapter")
                || err.to_string().to_lowercase().contains("configured"),
            "expected a no-adapter error, got: {err}",
        );
    }
}
