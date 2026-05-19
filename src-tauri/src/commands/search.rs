//! Full-text search command.

use cal_adapter_local::{LocalAdapter, SearchFilters, SearchResults};
use tauri::State;

use super::CommandResult;

#[tauri::command]
pub async fn search(
    adapter: State<'_, LocalAdapter>,
    query: String,
    filters: Option<SearchFilters>,
) -> CommandResult<SearchResults> {
    let filters = filters.unwrap_or_default();
    Ok(adapter.search(&query, &filters)?)
}
