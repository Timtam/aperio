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
    event_self_warm_needed, has_snapshot, is_stale, refresh_contacts, refresh_events,
    refresh_tasks, spawn_item_refresh, spawn_refresh, SWR_TTL_SECS,
};

/// Unwrap a snapshot-cache read to its rows, LOGGING a failure instead of
/// silently swallowing it. A failed read degrades one whole container to
/// empty for that response — exactly the silent shrink the startup
/// count-oscillation hunt traced — so it must at least be diagnosable in
/// the host log.
pub fn rows_or_logged_empty<T>(
    result: host_core::DbResult<Vec<T>>,
    what: &'static str,
    container: &str,
) -> Vec<T> {
    match result {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(
                target: "aperio::cache",
                what,
                container,
                ?err,
                "cache read failed; serving this container as EMPTY",
            );
            Vec::new()
        }
    }
}

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
