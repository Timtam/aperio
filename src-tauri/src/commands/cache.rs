//! External-cache control commands (CACHE-3).

use std::sync::Arc;

use tauri::State;

use super::CommandResult;
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
