//! Desktop entry point for the external-cache warm/periodic refresher.
//!
//! The refresher logic now lives in `host-core`
//! ([`host_core::cache::CacheRefresher`]) behind the
//! [`CacheObserver`](crate::cache::CacheObserver) seam, so the mobile
//! UniFFI host can reuse it without a scheduler. This module just builds
//! the desktop observer and, once the caller has installed what passes report
//! to, starts the periodic loop on Tauri's runtime.

use std::sync::Arc;

use tauri::{AppHandle, Runtime};

use crate::cache::{CacheObserver, CacheStore, RefreshCoordinator};
use crate::commands::cache_swr::TauriCacheObserver;
use crate::db::SharedConn;
use crate::registry::AdapterRegistry;

// Re-export the relocated types so existing `crate::cache_refresh::*`
// references (lib.rs, commands/cache.rs) keep resolving unchanged.
pub use crate::cache::{CacheRefreshStatus, CacheRefresher};

/// Build the desktop [`CacheRefresher`], wired to a [`TauriCacheObserver`]
/// so refresh progress reaches the existing `cache-updated` /
/// `cache-refresh-status` events. Not started yet: the caller installs what
/// every pass reports to (the contact sync) first, then calls [`start`].
/// Returns the shared handle so `refresh_external_cache` can drive a manual
/// pass through the same in-flight guard.
pub fn build<R: Runtime>(
    registry: Arc<AdapterRegistry>,
    cache: Arc<CacheStore>,
    coord: Arc<RefreshCoordinator>,
    db: SharedConn,
    app: AppHandle<R>,
) -> Arc<CacheRefresher> {
    let observer = Arc::new(TauriCacheObserver { app }) as Arc<dyn CacheObserver>;
    CacheRefresher::new(registry, cache, coord, db, observer)
}

/// Start the warm-on-boot + periodic loop on Tauri's async runtime.
pub fn start(refresher: &Arc<CacheRefresher>) {
    refresher.start_periodic(tauri::async_runtime::handle().inner());
}
