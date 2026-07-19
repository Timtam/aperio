//! External-cache control commands (CACHE-3).

use std::sync::Arc;

use tauri::State;

use super::{CommandError, CommandResult};
use crate::cache::CacheStore;
use crate::cache_refresh::{CacheRefreshStatus, CacheRefresher};

/// Kick an immediate background warm pass (manual "refresh now"). Returns
/// as soon as the pass is queued; progress arrives via the
/// `cache-refresh-status` / `cache-updated` events.
#[tauri::command]
pub fn refresh_external_cache(refresher: State<'_, Arc<CacheRefresher>>) -> CommandResult<()> {
    refresher.trigger();
    Ok(())
}

/// Current refresher status (whether a pass is running + the last
/// completed timestamp). Seeds the toolbar indicator on mount so it
/// reflects the persisted "last updated" before the first live event.
#[tauri::command]
pub fn get_cache_refresh_status(
    refresher: State<'_, Arc<CacheRefresher>>,
) -> CommandResult<CacheRefreshStatus> {
    Ok(refresher.status())
}

/// Every account's currently-failing containers (latest refresh attempt
/// recorded an error) — the data behind the per-account error surface,
/// so a wrong iCloud password shows up as a warning instead of silent
/// staleness. Refreshed by the frontend after each cache-refresh pass.
#[tauri::command]
pub fn get_refresh_errors(
    cache: State<'_, Arc<CacheStore>>,
) -> CommandResult<Vec<host_core::cache::AccountRefreshErrors>> {
    cache.refresh_errors().map_err(|e| CommandError {
        code: "cache_error",
        message: e.to_string(),
    })
}

/// Force a FULL cold re-sync of one external account: clear the delta tokens,
/// covered windows and freshness markers across all of its containers, then
/// kick a warm pass so each re-bootstraps from the provider. The cached rows
/// stay visible as an offline fallback until the cold fetch replaces them, and
/// credentials are untouched (no re-auth). The recovery action for a "stuck"
/// external cache — e.g. a CalDAV bootstrap that enumerated an incomplete
/// resource set yet persisted a sync-token, so later deltas reported "no
/// changes" over permanently-missing events.
#[tauri::command]
pub fn reset_account_sync(
    account_id: String,
    cache: State<'_, Arc<CacheStore>>,
    refresher: State<'_, Arc<CacheRefresher>>,
) -> CommandResult<()> {
    cache
        .reset_account_sync(&account_id)
        .map_err(CommandError::from)?;
    refresher.trigger();
    Ok(())
}
