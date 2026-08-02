//! Full-text search command.
//!
//! Merges two halves (§13.1 — search covers EVERY locally cached item):
//!   1. the LOCAL events/tasks tables via `adapter-local`'s FTS
//!      indexes, and
//!   2. the EXTERNAL snapshot cache (iCloud, Google, EWS, Vikunja,
//!      Todoist, …) via the `cache_*_fts` mirrors from migration 0027.
//!
//! Both halves consume the same prepared MATCH string, so prefix
//! semantics and filters behave identically. The cache half is
//! best-effort: a cache query error is logged and the local results
//! still come back, rather than failing the whole search.

use std::sync::Arc;

use adapter_local::{prepare_fts_query, LocalAdapter, SearchFilters, SearchResults};
use tauri::State;

use super::CommandResult;
use crate::cache::CacheStore;

#[tauri::command]
pub async fn search(
    adapter: State<'_, LocalAdapter>,
    cache: State<'_, Arc<CacheStore>>,
    query: String,
    filters: Option<SearchFilters>,
) -> CommandResult<SearchResults> {
    let filters = filters.unwrap_or_default();
    let mut results = adapter.search(&query, &filters)?;

    let fts = prepare_fts_query(&query);
    if !fts.is_empty() {
        match cache.search_events_fts(&fts, &filters) {
            Ok(events) => results.events.extend(events),
            Err(err) => tracing::warn!(?err, "external cache event search failed"),
        }
        match cache.search_tasks_fts(&fts, &filters) {
            Ok(tasks) => results.tasks.extend(tasks),
            Err(err) => tracing::warn!(?err, "external cache task search failed"),
        }
    }
    Ok(results)
}
