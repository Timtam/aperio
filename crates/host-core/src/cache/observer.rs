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

use super::{CacheUpdatedPayload, RefreshCause};

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

/// A warm pass's state, for the indicators and for the sentence that ends a
/// pass.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
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
    /// What the passes that just ended left undone: set only on the status
    /// that ends them (`refreshing` false), `None` on every other status and
    /// in a point-in-time query. `None` there too when it could not be read,
    /// so a surface falls back to its plain sentence instead of claiming
    /// that everything was updated.
    pub outcome: Option<PassOutcome>,
}

/// The accounts a pass could not update, for the sentence that ends it
/// ("updated, except: …", decision 180).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct PassOutcome {
    /// The same set the error surface shows (`CacheStore::refresh_errors`):
    /// confirmed failures only, so what is said is what is shown. Includes
    /// an account the pass could not try (its plugin is missing) whose
    /// error is still recorded: its data is not current either.
    pub failing: Vec<FailingAccount>,
    /// Every account the pass tried failed: nothing was updated, and "updated,
    /// except: everyone" would not be true.
    pub all_failed: bool,
}

/// One account a pass could not update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct FailingAccount {
    pub account_id: String,
    /// Its display name, as every surface shows it.
    pub name: String,
    pub cause: RefreshCause,
    /// `cause`'s severity, for comparing (see `AccountRefreshErrors::rank`).
    pub rank: u8,
}
