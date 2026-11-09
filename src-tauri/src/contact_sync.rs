//! Desktop contact sync (DESIGN.md §10.5).
//!
//! The contacts sync is the refresh of external data: every warm pass of the
//! [`CacheRefresher`] reads the contact lists alongside the calendars and task
//! lists, on the one refresh interval (`sync.intervalMinutes`, Settings →
//! General). The status and the done signal live in
//! [`host_core::contact_sync::ContactSyncCore`]; this module installs the core
//! into the refresher with [`TauriContactSyncObserver`], which forwards the
//! core's `contacts-synced` broadcast to the Tauri event of the same name, and
//! answers the contacts commands.
//!
//! ## Triggers
//!   1. **App start** — the refresher's first pass, the read-only directories
//!      included.
//!   2. **Periodic** — every refresh interval; the read-only directories only
//!      when `contacts.includeReadOnlyOnSync` is on.
//!   3. **Manual** — the `sync_contacts_now` command: a contacts-only pass,
//!      queued behind a running one.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Runtime};
use tracing::warn;

use host_core::cache::CacheRefresher;
use host_core::contact_sync::{ContactSyncCore, ContactSyncObserver, ContactsSyncedPayload};

use crate::db::SharedConn;

// Re-export the moved types/consts so existing desktop imports
// (`crate::contact_sync::…` in commands/contacts.rs) keep resolving — the same
// shim pattern host-core's lib.rs uses for its own re-exports.
pub use host_core::contact_sync::{
    ContactsSyncStatus, PREF_INCLUDE_READ_ONLY_ON_SYNC, PREF_LAST_SYNCED_AT,
};

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

/// The desktop's contact sync: the host-core status holder, installed into
/// the refresher, and the refresher that runs the passes. Constructed once at
/// startup and kept in Tauri State for the commands.
pub struct ContactSync {
    core: Arc<ContactSyncCore>,
    refresher: Arc<CacheRefresher>,
}

impl ContactSync {
    /// Install the contacts' status and done signal into `refresher`, so
    /// every pass reports the contacts it read.
    pub fn install<R: Runtime>(
        refresher: Arc<CacheRefresher>,
        db: SharedConn,
        app: AppHandle<R>,
    ) -> Arc<Self> {
        let core = ContactSyncCore::new(db);
        refresher.set_contact_sync(
            Arc::clone(&core),
            Arc::new(TauriContactSyncObserver { app }),
        );
        Arc::new(Self { core, refresher })
    }

    /// "Sync contacts now": a contacts-only pass. `true` when it ran within
    /// this call, `false` when it waits behind a running pass (its
    /// `contacts-synced` follows). Passes queued while it runs go on after
    /// it returns.
    pub async fn sync_now(&self, include_read_only: bool) -> bool {
        self.refresher.warm_contacts(include_read_only).await
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
