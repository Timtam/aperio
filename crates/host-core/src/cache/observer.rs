//! The seam that decouples the cache-refresh machinery from how a host
//! surfaces refresh progress to its UI.
//!
//! The desktop forwards these notifications to Tauri events
//! (`cache-updated` / `cache-refresh-status`); the mobile UniFFI host
//! forwards them across the FFI bridge. The SWR helpers
//! ([`super::swr`]) and the periodic [`super::refresh::CacheRefresher`]
//! both call into a `dyn CacheObserver` instead of talking to any
//! particular UI layer.

use serde::Serialize;

use super::CacheUpdatedPayload;

/// Host-supplied sink for cache-refresh notifications.
///
/// One container's snapshot just changed → `cache_updated`; a warm pass
/// started or finished → `refresh_status`. Implementations must be
/// cheap and non-blocking (the desktop one just emits a Tauri event).
pub trait CacheObserver: Send + Sync {
    /// A background refresh (or warm pass) wrote fresh data for one
    /// container, so the UI should invalidate the matching view.
    fn cache_updated(&self, payload: &CacheUpdatedPayload);

    /// A warm pass changed its running/last-completed state.
    fn refresh_status(&self, status: &CacheRefreshStatus);
}

/// Status snapshot consumed by the toolbar indicator.
#[derive(Debug, Clone, Serialize)]
pub struct CacheRefreshStatus {
    /// True while a warm pass is running.
    pub refreshing: bool,
    /// RFC3339 of the last completed pass, if any (survives restarts).
    pub last_refreshed_at: Option<String>,
    /// Containers the RUNNING pass will refresh (`None` outside a pass / before
    /// enumeration). Lets the UI show "fetched X of N" external-refresh progress.
    pub total_targets: Option<u32>,
    /// Containers refreshed so far in the running pass (`None` outside a pass).
    pub fetched_targets: Option<u32>,
}
