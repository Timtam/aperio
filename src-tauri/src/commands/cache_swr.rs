//! Desktop wiring for the shared stale-while-revalidate cache machinery.
//!
//! The SWR helpers (`spawn_refresh`, `is_stale`, `refresh_*`, …) and the
//! periodic [`host_core::cache::CacheRefresher`] now live in `host-core`
//! behind a [`CacheObserver`] seam. This module supplies the desktop
//! implementation of that seam — [`TauriCacheObserver`] forwards refresh
//! notifications to the existing Tauri events — and re-exports the host
//! helpers so the command call sites keep their `cache_swr::` paths.

use tauri::{AppHandle, Emitter, Runtime};

use crate::cache::{CacheObserver, CacheRefreshStatus, CacheUpdatedPayload};

// Re-export the host-core SWR helpers so existing `cache_swr::*` call
// sites across the command layer resolve unchanged.
pub use crate::cache::{
    has_snapshot, is_stale, refresh_contacts, refresh_events, refresh_tasks, spawn_item_refresh,
    spawn_refresh, SWR_TTL_SECS,
};

/// Desktop [`CacheObserver`]: forwards cache-refresh notifications to the
/// Tauri `cache-updated` / `cache-refresh-status` events the frontend
/// listens on (`CacheSyncListener.tsx` / `useCacheRefresh.ts`).
pub struct TauriCacheObserver<R: Runtime> {
    pub app: AppHandle<R>,
}

impl<R: Runtime> CacheObserver for TauriCacheObserver<R> {
    fn cache_updated(&self, payload: &CacheUpdatedPayload) {
        if let Err(err) = self.app.emit("cache-updated", payload) {
            tracing::warn!(target: "aperio::cache", ?err, "failed to emit cache-updated");
        }
    }

    fn refresh_status(&self, status: &CacheRefreshStatus) {
        let _ = self.app.emit("cache-refresh-status", status);
    }
}
