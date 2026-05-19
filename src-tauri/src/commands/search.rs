//! Full-text search command.

use cal_adapter_local::{LocalAdapter, SearchResults};
use tauri::State;

use super::CommandResult;

#[tauri::command]
pub async fn search(
    adapter: State<'_, LocalAdapter>,
    query: String,
) -> CommandResult<SearchResults> {
    Ok(adapter.search(&query)?)
}
