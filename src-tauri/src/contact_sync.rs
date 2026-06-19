//! Desktop contact-sync worker (DESIGN.md §10.5).
//!
//! The pure sync work + status + the `user_prefs` readers now live in
//! [`host_core::contact_sync::ContactSyncCore`]; this is the **desktop driver**:
//! a tokio worker loop (app-start + periodic ticks) plus
//! [`TauriContactSyncObserver`], which forwards the core's `contacts-synced`
//! broadcast to the Tauri event of the same name. It mirrors the
//! `SyncOrchestrator`(core) / `SyncScheduler`(loop) split and the
//! `cache_refresh` driver — one source of truth in host-core, both platforms
//! drive it. The mobile (cal-ffi) side drives the SAME core from foreground
//! triggers instead of this loop.
//!
//! ## Triggers
//!   1. **App start** — once, after [`APP_START_DELAY`], so the registry has
//!      materialised every account and the UI has painted.
//!   2. **Periodic** — every `contacts.syncIntervalMinutes` minutes (re-read on
//!      every tick so a settings change applies on the next pass).
//!   3. **Manual** — the `sync_contacts_now` command (which also wakes this
//!      loop so the interval clock resets from the manual kick).

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Runtime};
use tracing::{debug, info, warn};

use host_core::contact_sync::{ContactSyncCore, ContactSyncObserver, ContactsSyncedPayload};

use crate::db::SharedConn;
use crate::registry::AdapterRegistry;

// Re-export the moved types/consts so existing desktop imports
// (`crate::contact_sync::…` in commands/contacts.rs) keep resolving — the same
// shim pattern host-core's lib.rs uses for its own re-exports.
pub use host_core::contact_sync::{
    ContactsSyncStatus, PREF_INCLUDE_READ_ONLY_ON_SYNC, PREF_LAST_SYNCED_AT,
    PREF_SYNC_INTERVAL_MINUTES,
};

/// How long after app start the first sync pass kicks off. Lets the
/// Tauri main thread finish wiring everything up + lets the UI
/// paint before we start consuming bandwidth. Desktop-only — the
/// mobile side has no app-start loop.
const APP_START_DELAY: Duration = Duration::from_secs(5);

/// Desktop observer: forwards the core's broadcast to the Tauri
/// `contacts-synced` event the frontend listens for. Mirrors
/// `TauriCacheObserver`; preserves the exact event name + payload.
pub struct TauriContactSyncObserver<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ContactSyncObserver for TauriContactSyncObserver<R> {
    fn contacts_synced(&self, payload: &ContactsSyncedPayload) {
        if let Err(err) = self.app.emit("contacts-synced", payload) {
            warn!(?err, "failed to emit contacts-synced event");
        }
    }
}

/// Desktop contact-sync scheduler: the host-core core + a Tauri observer + a
/// background worker loop. Constructed once at startup; shared between Tauri
/// State (for the commands) and the worker.
pub struct ContactSyncScheduler {
    core: Arc<ContactSyncCore>,
    observer: Arc<dyn ContactSyncObserver>,
}

impl ContactSyncScheduler {
    /// Construct the scheduler + start its periodic worker. The returned `Arc`
    /// is shared between Tauri State (for the `sync_contacts_now` command) and
    /// the background worker.
    pub fn spawn<R: Runtime>(
        registry: Arc<AdapterRegistry>,
        db: SharedConn,
        app: AppHandle<R>,
    ) -> Arc<Self> {
        let core = ContactSyncCore::new(registry, db);
        let observer: Arc<dyn ContactSyncObserver> = Arc::new(TauriContactSyncObserver { app });
        let scheduler = Arc::new(Self {
            core: Arc::clone(&core),
            observer: Arc::clone(&observer),
        });

        let worker_core = Arc::clone(&core);
        let worker_observer = Arc::clone(&observer);
        let notify = core.notify_handle();
        tauri::async_runtime::spawn(async move {
            // App-start kick — let the UI settle for 5 s before pulling. The
            // directory pull (GAL etc.) honours the user's
            // `contacts.includeReadOnlyOnSync` pref so an opted-in user gets the
            // GAL on boot too; default-off keeps the boot cheap for everyone else.
            tokio::time::sleep(APP_START_DELAY).await;
            let include_ro = worker_core.read_include_read_only_on_sync();
            info!(include_ro, "running app-start contact sync pass");
            worker_core.run_sync(&*worker_observer, include_ro).await;

            // Periodic loop. Re-read both the interval and the include-read-only
            // pref on every tick so a settings change applies on the next pass.
            loop {
                let minutes = worker_core.read_interval_minutes();
                let dur = Duration::from_secs(u64::from(minutes) * 60);
                tokio::select! {
                    _ = tokio::time::sleep(dur) => {
                        let include_ro = worker_core.read_include_read_only_on_sync();
                        info!(?dur, include_ro, "periodic contact sync tick");
                        worker_core.run_sync(&*worker_observer, include_ro).await;
                    }
                    _ = notify.notified() => {
                        // Manual sync just landed; restart the interval clock
                        // without firing another pass (the manual call already
                        // did one).
                        debug!("contact sync interval reset by manual trigger");
                    }
                }
            }
        });
        scheduler
    }

    /// Run one sync pass through the core + the desktop observer. Returns
    /// `true` when the pass ran, `false` when deduped (another in flight).
    pub async fn run_sync(&self, include_read_only: bool) -> bool {
        self.core.run_sync(&*self.observer, include_read_only).await
    }

    /// The persisted "also pull read-only directories" toggle (delegates to the
    /// core) — the command uses it to resolve a `None` override.
    pub fn read_include_read_only_on_sync(&self) -> bool {
        self.core.read_include_read_only_on_sync()
    }

    /// Snapshot the current sync status (delegates to the core).
    pub fn status(&self) -> ContactsSyncStatus {
        self.core.status()
    }
}
