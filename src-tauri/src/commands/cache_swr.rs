//! Shared stale-while-revalidate helpers for external-adapter reads
//! (CACHE-1/2). The snapshot cache is served instantly; a deduplicated
//! background refresh repopulates it and pushes `cache-updated`.

use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter};

use crate::cache::{CacheStore, CacheUpdatedPayload, RefreshCoordinator, SyncScope, SyncState};

/// Freshness window before a cached snapshot triggers a background
/// refresh. Short enough to keep an open session current, long enough to
/// spare the network on rapid reads and to break the refresh →
/// `cache-updated` → invalidate → re-read feedback loop.
pub const SWR_TTL_SECS: i64 = 60;

/// Emit the `cache-updated` push so the frontend invalidates the
/// affected view after a refresh (or a write-through) changed the cache.
pub fn emit_cache_updated(app: &AppHandle, scope: SyncScope, account: &str, container: &str) {
    let payload = CacheUpdatedPayload {
        scope: scope.as_str().to_string(),
        account_id: account.to_string(),
        container_id: container.to_string(),
    };
    if let Err(err) = app.emit("cache-updated", payload) {
        tracing::warn!(target: "aperio::cache", ?err, "failed to emit cache-updated");
    }
}

/// True when a cached snapshot exists at all (a refresh has completed at
/// least once for this scope/container).
pub fn has_snapshot(state: &Option<SyncState>) -> bool {
    state.as_ref().and_then(|s| s.last_refreshed_at).is_some()
}

/// True when the snapshot is missing or older than `ttl_secs`.
pub fn is_stale(state: &Option<SyncState>, ttl_secs: i64) -> bool {
    match state.as_ref().and_then(|s| s.last_refreshed_at) {
        Some(t) => Utc::now().signed_duration_since(t) > chrono::Duration::seconds(ttl_secs),
        None => true,
    }
}

/// Spawn a deduplicated, fire-and-forget background refresh: `fetch`
/// pulls fresh data from the adapter, `write` persists it into the
/// snapshot cache, then a `cache-updated` push is emitted. On a fetch
/// failure the error is recorded via `mark_error` and the stale snapshot
/// is left in place. Deduplicated through the [`RefreshCoordinator`] so
/// concurrent reads of the same container don't stack refreshes.
pub fn spawn_refresh<T, Fut, Fetch, Write>(
    app: AppHandle,
    cache: Arc<CacheStore>,
    coord: Arc<RefreshCoordinator>,
    scope: SyncScope,
    account: String,
    container: String,
    fetch: Fetch,
    write: Write,
) where
    T: Send + 'static,
    Fut: Future<Output = cal_core::Result<Vec<T>>> + Send + 'static,
    Fetch: FnOnce() -> Fut + Send + 'static,
    Write: FnOnce(&CacheStore, &[T]) -> crate::db::DbResult<()> + Send + 'static,
{
    let key = format!("{}:{}:{}", scope.as_str(), account, container);
    if !coord.try_claim(&key) {
        return; // a refresh for this container is already in flight
    }
    tokio::spawn(async move {
        match fetch().await {
            Ok(items) => match write(&cache, &items) {
                Ok(()) => emit_cache_updated(&app, scope, &account, &container),
                Err(err) => {
                    tracing::warn!(target: "aperio::cache", ?err, "background refresh: cache write failed")
                }
            },
            Err(err) => {
                let _ = cache.mark_error(&account, scope, &container, &err.to_string());
                tracing::warn!(
                    target: "aperio::cache",
                    scope = scope.as_str(),
                    account = %account,
                    container = %container,
                    ?err,
                    "background refresh failed",
                );
            }
        }
        coord.release(&key);
    });
}
