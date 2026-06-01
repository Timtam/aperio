//! Shared stale-while-revalidate helpers for external-adapter reads
//! (CACHE-1/2). The snapshot cache is served instantly; a deduplicated
//! background refresh repopulates it and pushes `cache-updated`.

use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter};

use cal_core::{CalendarFeature, ContactsFeature, DateRange, TasksFeature};

use crate::cache::{
    CacheStore, CacheUpdatedPayload, Delta, RefreshCoordinator, SyncScope, SyncState,
};

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

/// Spawn a deduplicated background refresh whose body already writes the
/// cache (the delta-aware `refresh_*` helpers below). On success emits
/// `cache-updated`; on a genuine provider failure records `mark_error`.
pub fn spawn_item_refresh<F, Fut>(
    app: AppHandle,
    cache: Arc<CacheStore>,
    coord: Arc<RefreshCoordinator>,
    scope: SyncScope,
    account: String,
    container: String,
    refresh: F,
) where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = cal_core::Result<()>> + Send + 'static,
{
    let key = format!("{}:{}:{}", scope.as_str(), account, container);
    if !coord.try_claim(&key) {
        return;
    }
    tokio::spawn(async move {
        match refresh().await {
            Ok(()) => emit_cache_updated(&app, scope, &account, &container),
            Err(err) => {
                let _ = cache.mark_error(&account, scope, &container, &err.to_string());
                tracing::warn!(
                    target: "aperio::cache",
                    scope = scope.as_str(),
                    account = %account,
                    container = %container,
                    ?err,
                    "background item refresh failed",
                );
            }
        }
        coord.release(&key);
    });
}

// ── Delta-or-full refresh (CACHE-4) ───────────────────────────────────
//
// The single refresh path for item containers: try the adapter's
// incremental `get_*_delta`, fall back to a full fetch when the adapter
// returns `Unsupported` (the all-default state today, so behaviour is
// unchanged). Cache-write errors are best-effort (logged via the `_`);
// only a genuine provider/network error propagates so the caller can
// serve a stale snapshot.

/// Refresh one external calendar's events into the snapshot cache.
pub async fn refresh_events(
    cache: &CacheStore,
    ext: &dyn CalendarFeature,
    account: &str,
    calendar: &str,
    range: DateRange,
) -> cal_core::Result<()> {
    let state = cache
        .get_sync_state(account, SyncScope::Events, calendar)
        .ok()
        .flatten();
    let token = state.as_ref().and_then(|s| s.sync_token.clone());
    // Folder-complete cache: once a delta-capable calendar has been
    // fully synced, its window is unbounded (set below) and every view
    // range is served straight from the snapshot. We only force a
    // token-less FULL sync when the cached window doesn't yet cover the
    // requested range — i.e. the very first sync, or migrating an older
    // range-scoped snapshot left over from before this change. In that
    // case the adapter re-emits the whole folder and we widen the window
    // to unbounded; an incremental delta against a partial window would
    // silently miss everything that didn't change since the cookie.
    let covered = matches!(
        state.as_ref().map(|s| (s.window_start, s.window_end)),
        Some((Some(ws), Some(we))) if ws <= range.start && we >= range.end
    );
    let effective_token = if covered { token.as_deref() } else { None };
    match ext.get_events_delta(calendar, range, effective_token).await {
        Ok(cs) => {
            if cs.full_resync || effective_token.is_none() {
                // Folder-complete adapters (EWS/CalDAV) return the WHOLE
                // collection, so the snapshot now covers any range —
                // record an unbounded window. Range-scoped adapters
                // (Google/Graph) only fetched `range`, so the window must
                // stay bounded to that range or we'd serve empty for the
                // months we never fetched.
                let window = if cs.complete {
                    DateRange::new(DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC)
                } else {
                    range
                };
                let _ = cache.replace_calendar_events(account, calendar, window, &cs.changes);
                let _ = cache.set_token(
                    account,
                    SyncScope::Events,
                    calendar,
                    cs.new_token.as_deref(),
                );
            } else {
                let _ = cache.apply_events_delta(
                    account,
                    calendar,
                    &Delta {
                        changes: cs.changes,
                        deletions: cs.deletions,
                        new_token: cs.new_token,
                    },
                );
            }
            Ok(())
        }
        Err(cal_core::Error::Unsupported(_)) => {
            let events = ext.get_events(calendar, range).await?;
            let _ = cache.replace_calendar_events(account, calendar, range, &events);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Refresh one external task list into the snapshot cache.
pub async fn refresh_tasks(
    cache: &CacheStore,
    ext: &dyn TasksFeature,
    account: &str,
    list: &str,
) -> cal_core::Result<()> {
    let token = cache
        .get_sync_state(account, SyncScope::Tasks, list)
        .ok()
        .flatten()
        .and_then(|s| s.sync_token);
    match ext.get_tasks_delta(list, token.as_deref()).await {
        Ok(cs) => {
            if cs.full_resync || token.is_none() {
                let _ = cache.replace_list_tasks(account, list, &cs.changes);
                let _ = cache.set_token(account, SyncScope::Tasks, list, cs.new_token.as_deref());
            } else {
                let _ = cache.apply_tasks_delta(
                    account,
                    list,
                    &Delta {
                        changes: cs.changes,
                        deletions: cs.deletions,
                        new_token: cs.new_token,
                    },
                );
            }
            Ok(())
        }
        Err(cal_core::Error::Unsupported(_)) => {
            let tasks = ext.get_tasks(list).await?;
            let _ = cache.replace_list_tasks(account, list, &tasks);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Refresh one external contact list into the snapshot cache.
pub async fn refresh_contacts(
    cache: &CacheStore,
    ext: &dyn ContactsFeature,
    account: &str,
    list: &str,
) -> cal_core::Result<()> {
    let token = cache
        .get_sync_state(account, SyncScope::Contacts, list)
        .ok()
        .flatten()
        .and_then(|s| s.sync_token);
    match ext.get_contacts_delta(list, token.as_deref()).await {
        Ok(cs) => {
            if cs.full_resync || token.is_none() {
                let _ = cache.replace_list_contacts(account, list, &cs.changes);
                let _ =
                    cache.set_token(account, SyncScope::Contacts, list, cs.new_token.as_deref());
            } else {
                let _ = cache.apply_contacts_delta(
                    account,
                    list,
                    &Delta {
                        changes: cs.changes,
                        deletions: cs.deletions,
                        new_token: cs.new_token,
                    },
                );
            }
            Ok(())
        }
        Err(cal_core::Error::Unsupported(_)) => {
            let contacts = ext.get_contacts(list).await?;
            let _ = cache.replace_list_contacts(account, list, &contacts);
            Ok(())
        }
        Err(err) => Err(err),
    }
}
