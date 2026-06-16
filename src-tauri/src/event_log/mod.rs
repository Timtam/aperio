//! Event log for the cross-device sync layer (Phase Sb, DESIGN.md §19).
//!
//! The writer itself ([`EventLogWriter`]) now lives in the reusable
//! `sync-engine` crate so the same engine serves desktop and mobile; it's
//! re-exported here for the existing call sites. This module keeps the
//! desktop-only `load_or_mint_device_id` helper (it reads `user_prefs` via the
//! SQLite connection) plus the rest of the event-log machinery (applier,
//! orchestrator, compactor, snapshot, scheduler, onboarding).

pub mod desktop_hooks;
pub mod desktop_store;
pub mod onboarding;
pub mod scheduler;

/// The settings sync whitelist lives in the reusable `sync-engine` crate;
/// re-exported here so existing `crate::event_log::whitelist::…` paths
/// (the command layer, the snapshot builder) keep resolving unchanged.
pub use sync_engine::whitelist;

/// The event-log applier now lives in the reusable `sync-engine` crate;
/// re-exported so existing `crate::event_log::{EventLogApplier, …}` paths
/// resolve. Its DB-backed tests live in `applier_tests` (they need the
/// real DesktopSyncStore + LocalAdapter, so they can't move with it).
pub use sync_engine::{ApplyReport, EventLogApplier};

#[cfg(test)]
mod applier_tests;
/// The sync orchestrator + the round-coordination hooks trait now live in
/// the reusable `sync-engine` crate; the desktop hooks impl (meta
/// heartbeat, sound-asset sync, device-name cache, compaction audit) lives
/// here. All re-exported so existing `crate::event_log::{…}` paths resolve.
pub use desktop_hooks::DesktopSyncRoundHooks;
/// The snapshot builder lives in the reusable `sync-engine` crate; the
/// desktop SQLite-backed `SyncStore` it runs against lives here. Both are
/// re-exported so the existing `crate::event_log::{SnapshotBuilder, …}`
/// paths keep resolving.
pub use desktop_store::DesktopSyncStore;
pub use onboarding::{
    DeviceSummary, OnboardingReport, OnboardingService, SyncPreview, PREF_DEVICE_NAME,
    PREF_ONBOARDED,
};
pub use scheduler::{
    read_interval_minutes, SyncScheduler, SyncStatusPayload, DEFAULT_SYNC_INTERVAL_MINUTES,
    PREF_SYNC_INTERVAL_MINUTES,
};
pub use sync_engine::{AperioSnapshotBody, SnapshotApplyOutcome, SnapshotBuilder};
/// The compactor now lives in the reusable `sync-engine` crate; re-exported
/// so existing `crate::event_log::{Compactor, …}` paths keep resolving.
pub use sync_engine::{
    CompactionReport, Compactor, DEFAULT_MAX_AGE_DAYS, DEFAULT_MAX_BYTES, DEFAULT_MAX_LOGS,
    PREF_MAX_AGE_DAYS, PREF_MAX_BYTES, PREF_MAX_LOGS,
};
pub use sync_engine::{
    SyncOrchestrator, SyncRoundHooks, SyncRoundReport, SyncStatus, SYNC_CURSOR_PREF_KEY,
};

/// The event-log writer lives in the reusable `sync-engine` crate.
pub use sync_engine::EventLogWriter;

use sync_core::{DeviceId, DEVICE_ID_PREF_KEY};
use tracing::warn;

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// Load the persisted device id from `user_prefs`, or mint a fresh one and
/// persist it. Called once during `setup()` before the writer is spawned.
///
/// Free-standing (not tied to the writer's construction) because the id is
/// needed elsewhere too — the meta.json registrar, the snapshot generator, the
/// sync scheduler — so every consumer reads the same single source of truth.
/// It reads `user_prefs` via the SQLite connection, so it stays desktop-side;
/// the platform-agnostic [`EventLogWriter`] takes the id as a parameter.
///
/// On a write failure (only a SQLite I/O hiccup) we warn and continue with the
/// in-memory id; the next app start re-mints, which is the right "device looked
/// like a new install" behaviour for a corrupted DB.
pub fn load_or_mint_device_id(db: &SharedConn) -> DeviceId {
    let repo = UserPrefsRepo::new(db);
    match repo.get(DEVICE_ID_PREF_KEY) {
        Ok(Some(stored)) if !stored.is_empty() => DeviceId::from_string(stored),
        _ => {
            let fresh = DeviceId::new();
            if let Err(err) = repo.set(DEVICE_ID_PREF_KEY, fresh.as_str()) {
                warn!(
                    ?err,
                    "couldn't persist {DEVICE_ID_PREF_KEY}; falling back to a transient id",
                );
            }
            fresh
        }
    }
}
