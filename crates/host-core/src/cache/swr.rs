//! Shared stale-while-revalidate helpers for external-adapter reads
//! (CACHE-1/2). The snapshot cache is served instantly; a deduplicated
//! background refresh repopulates it and notifies the host via
//! [`CacheObserver::cache_updated`].

use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

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
/// snapshot cache, then the observer is notified — but ONLY when the
/// write reports that cached content actually changed, so no-op refreshes
/// don't trigger frontend reload waves. On a fetch failure the error is
/// recorded via `mark_error` and the stale snapshot is left in place.
/// Deduplicated through the [`RefreshCoordinator`] so concurrent reads of
/// the same container don't stack refreshes.
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
    Write: FnOnce(&CacheStore, &[T]) -> crate::db::DbResult<bool> + Send + 'static,
{
    let key = format!("{}:{}:{}", scope.as_str(), account, container);
    if !coord.try_claim(&key) {
        return; // a refresh for this container is already in flight
    }
    rt.spawn(async move {
        match fetch().await {
            Ok(items) => match write(&cache, &items) {
                Ok(true) => observer.cache_updated(&CacheUpdatedPayload {
                    scope: scope.as_str().to_string(),
                    account_id: account.clone(),
                    container_id: container.clone(),
                }),
                Ok(false) => {} // content identical — nothing for the UI to reload
                Err(err) => {
                    tracing::warn!(target: "aperio::cache", ?err, "background refresh: cache write failed")
                }
            },
            Err(err) => {
                let _ = cache.mark_error(&account, scope, &container, &err.to_string(), false);
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
/// cache (the delta-aware `refresh_*` helpers below). Notifies the
/// observer only when the refresh reports changed content (no-op
/// refreshes stay UI-silent); on a genuine provider failure records
/// `mark_error`.
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
    Fut: Future<Output = cal_core::Result<bool>> + Send + 'static,
{
    let key = format!("{}:{}:{}", scope.as_str(), account, container);
    if !coord.try_claim(&key) {
        return;
    }
    rt.spawn(async move {
        match refresh().await {
            Ok(true) => observer.cache_updated(&CacheUpdatedPayload {
                scope: scope.as_str().to_string(),
                account_id: account.clone(),
                container_id: container.clone(),
            }),
            Ok(false) => {} // content identical — nothing for the UI to reload
            Err(err) => {
                let _ = cache.mark_error(&account, scope, &container, &err.to_string(), false);
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
///
/// Returns whether the cached content actually CHANGED (`false` for a
/// no-op delta, a byte-identical full re-fetch, or a write dropped by the
/// generation guard), so callers can skip the `cache-updated`
/// notification — the frontend must not reload on refreshes that changed
/// nothing.
pub async fn refresh_events(
    cache: &CacheStore,
    ext: &dyn CalendarFeature,
    account: &str,
    calendar: &str,
    range: DateRange,
) -> cal_core::Result<bool> {
    // Widen the requested view range to whole months before fetching. A
    // range-scoped adapter (Google/Graph) bakes `timeMin`/`timeMax` into its delta
    // token, so the token only ever covers what THIS fetch asked for. Fetching the
    // exact day/week on screen therefore makes every navigation a coverage miss
    // that re-fetches and clobbers the previous range (the day-to-day thrash).
    // Caching the surrounding month(s) instead makes the token AND the recorded
    // window span the whole block, so panning within it is served from the
    // snapshot and stays incrementally fresh. Folder-complete adapters (CalDAV/EWS)
    // return the whole collection regardless of the range, so widening is a
    // harmless no-op for them.
    let range = snap_to_month_window(range);
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
    // A full resync ends in `replace_calendar_events`, which clobbers the
    // calendar's ENTIRE cached set and records only the fetched window.
    // Fetching just the view range would shrink a warm −3…+12-month cache
    // down to the month on screen (visible as the day's entries collapsing
    // and re-growing when the next wide warm pass restores them). The DELTA
    // call therefore always carries the wide window — computed ONCE, so the
    // recorded window below is exactly what was fetched:
    //   - token-less bootstrap: the full set is fetched (and, on
    //     range-scoped adapters, tokenised) over the wide window;
    //   - token present: range-scoped adapters ignore the range on the
    //     token path (Google forbids timeMin with syncToken; Graph's
    //     deltaLink carries its own window; sync-collection/EWS never look
    //     at it) — EXCEPT when the provider answers with an unexpected
    //     full resync (Google 410 re-sync, CalDAV ctag change re-list): the
    //     response then spans the range we sent, and because we sent the
    //     wide one it can be written directly. (An earlier revision sent
    //     the narrow view range and RE-FETCHED wide on that answer — which
    //     doubled every ctag-detected change on ctag-only CalDAV servers.)
    let fetch_range = full_resync_range(range, Utc::now());
    // Snapshot the generation before the fetch (see refresh_tasks): drop a stale
    // write if a local mutation invalidates this calendar mid-fetch.
    let gen = cache.refresh_generation(account, SyncScope::Events, calendar);
    match ext
        .get_events_delta(calendar, fetch_range, effective_token)
        .await
    {
        Ok(cs) => {
            if cache.refresh_generation(account, SyncScope::Events, calendar) != gen {
                return Ok(false);
            }
            if cs.full_resync || effective_token.is_none() {
                // Folder-complete adapters (EWS/CalDAV) return the WHOLE
                // collection, so the snapshot now covers any range —
                // record an unbounded window. Range-scoped adapters
                // (Google/Graph) only fetched `fetch_range`, so the window
                // must stay bounded to that range or we'd serve empty for
                // the months we never fetched.
                let window = if cs.complete {
                    unbounded_window()
                } else {
                    fetch_range
                };
                // Resources the adapter enumerated but could not FETCH
                // (`unfetched`, e.g. CalDAV multiget skips) are absent
                // from `changes` without having been deleted — carry
                // their previously cached rows into the replace so a
                // partially-failing bootstrap can only ever GROW the
                // snapshot toward the true set, instead of jittering the
                // visible count with each retry's different skip set.
                let mut changes = cs.changes;
                if !cs.unfetched.is_empty() {
                    let preserved = cache
                        .read_events_by_native(account, calendar, &cs.unfetched)
                        .unwrap_or_default();
                    let kept: Vec<_> = {
                        let fetched_ids: std::collections::HashSet<&str> =
                            changes.iter().map(|e| e.id.as_str()).collect();
                        preserved
                            .into_iter()
                            .filter(|e| !fetched_ids.contains(e.id.as_str()))
                            .collect()
                    };
                    changes.extend(kept);
                }
                let changed = cache
                    .replace_calendar_events(account, calendar, window, &changes)
                    .unwrap_or(true);
                let _ = cache.set_token(
                    account,
                    SyncScope::Events,
                    calendar,
                    cs.new_token.as_deref(),
                );
                Ok(changed)
            } else {
                Ok(cache
                    .apply_events_delta(
                        account,
                        calendar,
                        &Delta {
                            changes: cs.changes,
                            deletions: cs.deletions,
                            new_token: cs.new_token,
                        },
                    )
                    .unwrap_or(true))
            }
        }
        Err(cal_core::Error::Unsupported(_)) => {
            // No delta support (device EventKit, iCal): EVERY refresh is a
            // full fetch, so fetching the wide window here would repeat a
            // 15-month expansion on each stale view read. Fetch only the
            // (month-snapped) view range and write it through the
            // RANGE-SCOPED replace, which touches just the overlapping rows
            // and unions the recorded window — no clobber of the warm
            // cache, no per-read wide fetch. The warm pass still passes the
            // wide window explicitly, which keeps the whole swath fresh
            // (and reconciles out-of-range deletions) on its own cadence.
            let events = ext.get_events(calendar, range).await?;
            if cache.refresh_generation(account, SyncScope::Events, calendar) != gen {
                return Ok(false);
            }
            Ok(cache
                .replace_calendar_events_in_range(account, calendar, range, &events)
                .unwrap_or(true))
        }
        Err(err) => Err(err),
    }
}

/// The window a FULL resync rebuilds: the warm pass's rolling window
/// (−[`refresh::WINDOW_PAST_DAYS`]…+[`refresh::WINDOW_FUTURE_DAYS`] around
/// `now`) united with the requested view range, so a navigation outside
/// the rolling window is still covered. A full resync replaces the
/// calendar's ENTIRE cached set and records only the fetched window — a
/// narrower fetch would shrink whatever wider window the warm pass had
/// built (visible as day counts collapsing until the next wide pass).
///
/// Endpoints are truncated to WHOLE SECONDS: the range crosses the mobile
/// FFI as RFC-3339 strings, and the iOS EventKit bridge's ISO-8601 parser
/// rejects fractional seconds — a `Utc::now()`-precision endpoint made
/// every device-calendar refresh throw "invalid event range".
fn full_resync_range(view: DateRange, now: DateTime<Utc>) -> DateRange {
    let now = now.with_nanosecond(0).unwrap_or(now);
    let wide_start = now - Duration::days(super::refresh::WINDOW_PAST_DAYS);
    let wide_end = now + Duration::days(super::refresh::WINDOW_FUTURE_DAYS);
    DateRange::new(
        truncate_subsec(view.start.min(wide_start)),
        truncate_subsec(view.end.max(wide_end)),
    )
}

/// Drop sub-second precision (see [`full_resync_range`]).
fn truncate_subsec(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt.with_nanosecond(0).unwrap_or(dt)
}

/// Expand a view-sized range to whole-month UTC boundaries so a range-scoped
/// adapter caches (and tokenises) a block wide enough that day/week panning within
/// it stays a snapshot hit. A range already wider than ~13 months — a
/// folder-complete unbounded fetch, or an unusually large custom range — is left
/// untouched. Never shrinks the requested range.
fn snap_to_month_window(range: DateRange) -> DateRange {
    if range.end <= range.start || range.end - range.start > Duration::days(400) {
        return range;
    }
    let start = month_start(range.start);
    let end = month_ceil(range.end);
    if start <= range.start && end >= range.end {
        DateRange::new(start, end)
    } else {
        // Defensive: any pathological calendar arithmetic → keep the exact range.
        range
    }
}

/// First instant (UTC) of `dt`'s month.
fn month_start(dt: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(dt)
}

/// Start of the month at or after `dt`: `dt` itself when it is already a month
/// start, else the first instant of the following month.
fn month_ceil(dt: DateTime<Utc>) -> DateTime<Utc> {
    let start = month_start(dt);
    if start == dt {
        return dt;
    }
    let (year, month) = if dt.month() == 12 {
        (dt.year() + 1, 1)
    } else {
        (dt.year(), dt.month() + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .unwrap_or(dt)
}

/// Refresh one external task list into the snapshot cache. Returns whether
/// cached content changed (see [`refresh_events`]).
pub async fn refresh_tasks(
    cache: &CacheStore,
    ext: &dyn TasksFeature,
    account: &str,
    list: &str,
) -> cal_core::Result<bool> {
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
                return Ok(false);
            }
            if cs.full_resync || token.is_none() {
                let changed = cache
                    .replace_list_tasks(account, list, &cs.changes)
                    .unwrap_or(true);
                let _ = cache.set_token(account, SyncScope::Tasks, list, cs.new_token.as_deref());
                Ok(changed)
            } else {
                Ok(cache
                    .apply_tasks_delta(
                        account,
                        list,
                        &Delta {
                            changes: cs.changes,
                            deletions: cs.deletions,
                            new_token: cs.new_token,
                        },
                    )
                    .unwrap_or(true))
            }
        }
        Err(cal_core::Error::Unsupported(_)) => {
            let tasks = ext.get_tasks(list).await?;
            if cache.refresh_generation(account, SyncScope::Tasks, list) != gen {
                return Ok(false);
            }
            Ok(cache
                .replace_list_tasks(account, list, &tasks)
                .unwrap_or(true))
        }
        Err(err) => Err(err),
    }
}

/// Refresh one external task list's SECTIONS into the snapshot cache.
///
/// Sections have no provider delta path (`TasksFeature::list_sections`
/// always returns the full set), so this is a straight full-fetch +
/// `replace_sections` — no token, no delta merge. The generation guard is
/// kept identical to `refresh_tasks`: snapshot the container's refresh
/// generation before the (slow) fetch and drop the write if a local
/// mutation invalidated the list mid-fetch, so we can't clobber the
/// invalidation with a pre-mutation snapshot.
pub async fn refresh_sections(
    cache: &CacheStore,
    ext: &dyn TasksFeature,
    account: &str,
    list: &str,
) -> cal_core::Result<bool> {
    let gen = cache.refresh_generation(account, SyncScope::Sections, list);
    let sections = ext.list_sections(list).await?;
    if cache.refresh_generation(account, SyncScope::Sections, list) != gen {
        return Ok(false);
    }
    Ok(cache
        .replace_sections(account, list, &sections)
        .unwrap_or(true))
}

/// Refresh one external contact list into the snapshot cache. Returns
/// whether cached content changed (see [`refresh_events`]).
pub async fn refresh_contacts(
    cache: &CacheStore,
    ext: &dyn ContactsFeature,
    account: &str,
    list: &str,
) -> cal_core::Result<bool> {
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
                return Ok(false);
            }
            if cs.full_resync || token.is_none() {
                let changed = cache
                    .replace_list_contacts(account, list, &cs.changes)
                    .unwrap_or(true);
                let _ =
                    cache.set_token(account, SyncScope::Contacts, list, cs.new_token.as_deref());
                Ok(changed)
            } else {
                Ok(cache
                    .apply_contacts_delta(
                        account,
                        list,
                        &Delta {
                            changes: cs.changes,
                            deletions: cs.deletions,
                            new_token: cs.new_token,
                        },
                    )
                    .unwrap_or(true))
            }
        }
        Err(cal_core::Error::Unsupported(_)) => {
            let contacts = ext.get_contacts(list).await?;
            if cache.refresh_generation(account, SyncScope::Contacts, list) != gen {
                return Ok(false);
            }
            Ok(cache
                .replace_list_contacts(account, list, &contacts)
                .unwrap_or(true))
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

    fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn snap_widens_a_single_day_to_its_whole_month() {
        // A Wednesday in the middle of June → the whole of June.
        let snapped = snap_to_month_window(DateRange::new(at(2026, 6, 17), at(2026, 6, 18)));
        assert_eq!(snapped.start, at(2026, 6, 1));
        assert_eq!(snapped.end, at(2026, 7, 1));
    }

    #[test]
    fn snap_maps_every_day_of_a_month_to_the_same_window() {
        // The whole point: day-to-day panning within a month lands on ONE window,
        // so it's a snapshot hit instead of a re-fetch each day.
        let june = snap_to_month_window(DateRange::new(at(2026, 6, 1), at(2026, 6, 2)));
        for day in [3, 15, 30] {
            let start = at(2026, 6, day);
            let other = snap_to_month_window(DateRange::new(start, start + Duration::days(1)));
            assert_eq!(other.start, june.start);
            assert_eq!(other.end, june.end);
        }
    }

    #[test]
    fn snap_spans_both_months_for_a_cross_month_week() {
        // A week Dec 28 2026 .. Jan 4 2027 → Dec 1 .. Feb 1 (both months).
        let snapped = snap_to_month_window(DateRange::new(at(2026, 12, 28), at(2027, 1, 4)));
        assert_eq!(snapped.start, at(2026, 12, 1));
        assert_eq!(snapped.end, at(2027, 2, 1));
    }

    #[test]
    fn snap_leaves_month_aligned_ends_alone() {
        // A month view whose exclusive end is already a month boundary must not
        // over-extend into the next month.
        let snapped = snap_to_month_window(DateRange::new(at(2026, 6, 1), at(2026, 7, 1)));
        assert_eq!(snapped.start, at(2026, 6, 1));
        assert_eq!(snapped.end, at(2026, 7, 1));
    }

    #[test]
    fn snap_leaves_a_huge_range_untouched() {
        // A folder-complete unbounded fetch (or an outsized custom range) is not
        // widened — snapping is only for view-sized ranges.
        let huge = DateRange::new(at(2020, 1, 1), at(2030, 1, 1));
        let snapped = snap_to_month_window(huge);
        assert_eq!(snapped.start, huge.start);
        assert_eq!(snapped.end, huge.end);
    }

    #[test]
    fn full_resync_range_covers_the_warm_window() {
        // A view inside the rolling window → the full warm-pass window, so
        // a token-less rebuild can't shrink the cache below what the warm
        // pass maintains.
        let now = at(2026, 6, 15);
        let view = DateRange::new(at(2026, 6, 1), at(2026, 7, 1));
        let wide = full_resync_range(view, now);
        assert_eq!(wide.start, now - Duration::days(92));
        assert_eq!(wide.end, now + Duration::days(366));
    }

    #[test]
    fn full_resync_range_truncates_subseconds() {
        // The endpoints cross the mobile FFI as RFC-3339 strings and the iOS
        // bridge's ISO-8601 parser rejects fractional seconds — a raw
        // Utc::now() endpoint broke every device-calendar refresh.
        let now = at(2026, 6, 15) + Duration::nanoseconds(123_456_789);
        let view = DateRange::new(at(2026, 6, 1), at(2026, 7, 1));
        let wide = full_resync_range(view, now);
        assert_eq!(wide.start.timestamp_subsec_nanos(), 0);
        assert_eq!(wide.end.timestamp_subsec_nanos(), 0);
        assert_eq!(wide.start, at(2026, 6, 15) - Duration::days(92));
        assert_eq!(wide.end, at(2026, 6, 15) + Duration::days(366));
    }

    #[test]
    fn full_resync_range_extends_to_an_out_of_window_view() {
        // Navigating years ahead: the rebuild must still cover the view.
        let now = at(2026, 6, 15);
        let view = DateRange::new(at(2030, 1, 1), at(2030, 2, 1));
        let wide = full_resync_range(view, now);
        assert_eq!(wide.start, now - Duration::days(92));
        assert_eq!(wide.end, at(2030, 2, 1));
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
