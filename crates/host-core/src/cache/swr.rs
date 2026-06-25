//! Shared stale-while-revalidate helpers for external-adapter reads
//! (CACHE-1/2). The snapshot cache is served instantly; a deduplicated
//! background refresh repopulates it and notifies the host via
//! [`CacheObserver::cache_updated`].

use std::future::Future;
use std::sync::Arc;

use chrono::Utc;

use cal_core::{CalendarFeature, ContactsFeature, DateRange, TasksFeature};

use super::{
    unbounded_window, CacheObserver, CacheStore, CacheUpdatedPayload, Delta, RefreshCoordinator,
    SyncScope, SyncState,
};

/// Freshness window before a cached snapshot triggers a background
/// refresh. Short enough to keep an open session current, long enough to
/// spare the network on rapid reads and to break the refresh →
/// `cache-updated` → invalidate → re-read feedback loop.
pub const SWR_TTL_SECS: i64 = 60;

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

/// Cooldown (seconds) suppressing a coverage-miss event self-warm for a
/// calendar refreshed this recently — see [`event_self_warm_needed`].
const COVERAGE_REFRESH_COOLDOWN_SECS: i64 = 5;

/// Whether a cached event read should kick a background self-warm. Shared by
/// the desktop `get_events` command and the mobile cal-ffi host so both
/// self-warm IDENTICALLY (one source of truth for the loop-prevention below).
///
/// Refresh when the snapshot is STALE, or when its window doesn't COVER the
/// requested `range` — EXCEPT skip the coverage-miss case for a calendar
/// refreshed within [`COVERAGE_REFRESH_COOLDOWN_SECS`]. On a COLD cache every
/// read is a coverage miss, and a refresh's `cache-updated` makes the host
/// re-read all calendars; without the cooldown the still-cold calendars
/// re-spawn on every re-read — a sub-second feedback loop (the coverage branch
/// is intentionally NOT TTL-gated, so `SWR_TTL_SECS` never bounds it; a warm
/// cache simply has no uncovered calendars to fuel it, which is why the loop
/// only appears on a freshly-wiped cache). A just-refreshed calendar's window
/// was just written, so an immediate re-fetch can't change coverage; skip it
/// briefly. `stale` still refreshes on its own cadence, and a genuine range
/// navigation refreshes once the cooldown lapses.
pub fn event_self_warm_needed(state: &Option<SyncState>, range: DateRange) -> bool {
    if is_stale(state, SWR_TTL_SECS) {
        return true;
    }
    let covers = matches!(
        state.as_ref().map(|s| (s.window_start, s.window_end)),
        Some((Some(ws), Some(we))) if ws <= range.start && we >= range.end
    );
    if covers {
        return false;
    }
    let recently_refreshed = state
        .as_ref()
        .and_then(|s| s.last_refreshed_at)
        .is_some_and(|t| {
            Utc::now().signed_duration_since(t)
                < chrono::Duration::seconds(COVERAGE_REFRESH_COOLDOWN_SECS)
        });
    !recently_refreshed
}

/// Spawn a deduplicated, fire-and-forget background refresh: `fetch`
/// pulls fresh data from the adapter, `write` persists it into the
/// snapshot cache, then the observer is notified. On a fetch
/// failure the error is recorded via `mark_error` and the stale snapshot
/// is left in place. Deduplicated through the [`RefreshCoordinator`] so
/// concurrent reads of the same container don't stack refreshes.
pub fn spawn_refresh<T, Fut, Fetch, Write>(
    rt: &tokio::runtime::Handle,
    observer: Arc<dyn CacheObserver>,
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
    rt.spawn(async move {
        match fetch().await {
            Ok(items) => match write(&cache, &items) {
                Ok(()) => observer.cache_updated(&CacheUpdatedPayload {
                    scope: scope.as_str().to_string(),
                    account_id: account.clone(),
                    container_id: container.clone(),
                }),
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
/// cache (the delta-aware `refresh_*` helpers below). On success notifies
/// the observer; on a genuine provider failure records `mark_error`.
pub fn spawn_item_refresh<F, Fut>(
    rt: &tokio::runtime::Handle,
    observer: Arc<dyn CacheObserver>,
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
    rt.spawn(async move {
        match refresh().await {
            Ok(()) => observer.cache_updated(&CacheUpdatedPayload {
                scope: scope.as_str().to_string(),
                account_id: account.clone(),
                container_id: container.clone(),
            }),
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
    // Snapshot the generation before the fetch (see refresh_tasks): drop a stale
    // write if a local mutation invalidates this calendar mid-fetch.
    let gen = cache.refresh_generation(account, SyncScope::Events, calendar);
    match ext.get_events_delta(calendar, range, effective_token).await {
        Ok(cs) => {
            if cache.refresh_generation(account, SyncScope::Events, calendar) != gen {
                return Ok(());
            }
            if cs.full_resync || effective_token.is_none() {
                // Folder-complete adapters (EWS/CalDAV) return the WHOLE
                // collection, so the snapshot now covers any range —
                // record an unbounded window. Range-scoped adapters
                // (Google/Graph) only fetched `range`, so the window must
                // stay bounded to that range or we'd serve empty for the
                // months we never fetched.
                let window = if cs.complete {
                    unbounded_window()
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
            if cache.refresh_generation(account, SyncScope::Events, calendar) != gen {
                return Ok(());
            }
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
    // Snapshot the generation BEFORE the (slow) fetch. If a local mutation
    // invalidates this list while we fetch (e.g. a task completed in the
    // day-start review), the generation changes and we drop our now-stale write
    // so it can't clobber the invalidation with the pre-mutation snapshot.
    let gen = cache.refresh_generation(account, SyncScope::Tasks, list);
    let token = cache
        .get_sync_state(account, SyncScope::Tasks, list)
        .ok()
        .flatten()
        .and_then(|s| s.sync_token);
    match ext.get_tasks_delta(list, token.as_deref()).await {
        Ok(cs) => {
            if cache.refresh_generation(account, SyncScope::Tasks, list) != gen {
                return Ok(());
            }
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
            if cache.refresh_generation(account, SyncScope::Tasks, list) != gen {
                return Ok(());
            }
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
    // Snapshot the generation before the fetch (see refresh_tasks): drop a stale
    // write if a local mutation invalidates this list mid-fetch.
    let gen = cache.refresh_generation(account, SyncScope::Contacts, list);
    let token = cache
        .get_sync_state(account, SyncScope::Contacts, list)
        .ok()
        .flatten()
        .and_then(|s| s.sync_token);
    match ext.get_contacts_delta(list, token.as_deref()).await {
        Ok(cs) => {
            if cache.refresh_generation(account, SyncScope::Contacts, list) != gen {
                return Ok(());
            }
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
            if cache.refresh_generation(account, SyncScope::Contacts, list) != gen {
                return Ok(());
            }
            let _ = cache.replace_list_contacts(account, list, &contacts);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// A `SyncState` refreshed `refreshed_secs_ago` seconds ago, with an optional
    /// window expressed as day offsets from now.
    fn state(refreshed_secs_ago: i64, window_days: Option<(i64, i64)>) -> Option<SyncState> {
        let now = Utc::now();
        Some(SyncState {
            last_refreshed_at: Some(now - Duration::seconds(refreshed_secs_ago)),
            window_start: window_days.map(|(s, _)| now + Duration::days(s)),
            window_end: window_days.map(|(_, e)| now + Duration::days(e)),
            ..Default::default()
        })
    }

    /// The viewed range: now .. now + 1 day.
    fn range() -> DateRange {
        let now = Utc::now();
        DateRange::new(now, now + Duration::days(1))
    }

    #[test]
    fn cold_state_warms() {
        // No snapshot at all → stale → warm (the genuine first cold load).
        assert!(event_self_warm_needed(&None, range()));
    }

    #[test]
    fn fresh_and_covered_does_not_warm() {
        // Refreshed 1s ago, window -1..+2 days covers the 0..+1 view.
        assert!(!event_self_warm_needed(&state(1, Some((-1, 2))), range()));
    }

    #[test]
    fn fresh_uncovered_within_cooldown_does_not_warm() {
        // Refreshed 1s ago (inside the cooldown), window +10..+20 days misses the
        // view — this is the cold-cache loop case the cooldown suppresses.
        assert!(!event_self_warm_needed(&state(1, Some((10, 20))), range()));
    }

    #[test]
    fn fresh_uncovered_past_cooldown_warms() {
        // Refreshed 10s ago (past the 5s cooldown, still < 60s TTL), uncovered →
        // a genuine navigation to an out-of-window range refreshes.
        assert!(event_self_warm_needed(&state(10, Some((10, 20))), range()));
    }

    #[test]
    fn stale_warms_even_if_covered() {
        // Refreshed 120s ago (> 60s TTL) → stale regardless of coverage.
        assert!(event_self_warm_needed(&state(120, Some((-1, 2))), range()));
    }
}
