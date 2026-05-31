import { useEffect, useMemo, useState } from 'react';

import { getEvents } from '../api/client';
import type { CalendarEvent } from '../api/types';
import { expandAll } from '../intl/recurrence';
import { useCalendarStore } from './CalendarStore';
import { useDialogState } from './DialogState';
import { useViewState } from './ViewState';

/**
 * Pull events from every selected calendar in a given UTC range and
 * return the aggregated, chronologically sorted list.
 *
 * Phase 3 fans out one `get_events` per calendar in parallel. With local
 * calendars this is fast enough; when external adapters arrive in
 * Phase 6 each one already does its own range fetch, so the shape stays
 * the same.
 *
 * Stale-while-revalidate cache:
 *
 *   Switching views (Day → Week → Month) used to flash the "Lädt …"
 *   indicator because each view mounts a fresh `useEvents` and re-runs
 *   the full backend fan-out — over CalDAV that's a 300–800 ms wait,
 *   well above the deferred-loading threshold.
 *
 *   The module-level `eventsCache` below holds the last result for
 *   each `(calendarIds, range)` key. When a hook initialises (or its
 *   range changes), the cache is read synchronously; if there is a
 *   matching entry the events come back instantly with `loading=false`
 *   and the indicator never appears. A background fetch still fires
 *   to refresh the data — the cached entry is replaced when the new
 *   batch lands.
 *
 *   `dataVersion` is the global "data may have changed" counter
 *   (bumped after every dialog close and explicit `invalidateData()`).
 *   When the cache observes a newer version it clears wholesale —
 *   conservative but correct, mirrors what `dataVersion` was designed
 *   to do.
 */

/** Cache key: sorted calendar IDs joined with `,` + range ISO strings. */
type CacheKey = string;

const eventsCache = new Map<CacheKey, CalendarEvent[]>();
let cachedDataVersion = -1;

function cacheKey(idsKey: string, startIso: string, endIso: string): CacheKey {
  return `${idsKey}|${startIso}|${endIso}`;
}

function ensureCacheVersion(version: number): void {
  if (version !== cachedDataVersion) {
    eventsCache.clear();
    cachedDataVersion = version;
  }
}

function cacheGet(
  key: CacheKey,
  version: number,
): CalendarEvent[] | undefined {
  ensureCacheVersion(version);
  return eventsCache.get(key);
}

function cacheSet(
  key: CacheKey,
  version: number,
  events: CalendarEvent[],
): void {
  ensureCacheVersion(version);
  eventsCache.set(key, events);
}

/** Test-only escape hatch — wipes the cache so each test starts clean. */
export function __resetEventsCacheForTests(): void {
  eventsCache.clear();
  cachedDataVersion = -1;
}

export function useEvents(range: { start: Date; end: Date }) {
  const { selectedCalendarIds, calendars, loading: storeLoading } =
    useCalendarStore();
  const { dataVersion } = useDialogState();
  const { focusedCalendarId } = useViewState();

  // Stabilise the range to ISO strings so we only re-fetch when the
  // boundary actually moves, not on every render that re-creates a Date.
  const startIso = useMemo(() => range.start.toISOString(), [range.start]);
  const endIso = useMemo(() => range.end.toISOString(), [range.end]);
  // Effective calendar set: focus mode collapses the multi-select
  // sidebar to a single calendar without disturbing the user's
  // checked-on/off state. When focus exits, this reverts to the
  // normal selected set and the cache key swaps accordingly — fetches
  // hit the existing SWR cache because the multi-calendar key still
  // matches its prior entries.
  const effectiveIds = useMemo<string[]>(() => {
    if (focusedCalendarId) return [focusedCalendarId];
    return [...selectedCalendarIds];
  }, [focusedCalendarId, selectedCalendarIds]);
  const idsKey = useMemo(
    () => [...effectiveIds].sort().join(' '),
    [effectiveIds],
  );
  const key = cacheKey(idsKey, startIso, endIso);

  // Lazy initialisers: the very first render already reflects the
  // cache when the key has been seen before in this session. No
  // flicker between empty and cached state.
  const [events, setEvents] = useState<CalendarEvent[]>(
    () => cacheGet(key, dataVersion) ?? [],
  );
  const [loading, setLoading] = useState<boolean>(
    () => cacheGet(key, dataVersion) === undefined,
  );
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);

    // Cache check on every (re)run. If the key changed or the
    // dataVersion ticked, this is where state snaps to the cached
    // batch (cache hit, loading stays / flips false) or arms a real
    // fetch (cache miss).
    const cached = cacheGet(key, dataVersion);
    const hadCache = cached !== undefined;
    if (cached) {
      setEvents(cached);
      setLoading(false);
    } else {
      setLoading(true);
    }

    // Defer the actual network call until the calendar catalog is
    // ready. The cache short-circuit above already runs unconditionally
    // — having stale events on screen is fine while the store still
    // settles.
    if (storeLoading) return;

    const ids = effectiveIds;
    if (ids.length === 0) {
      setEvents([]);
      setLoading(false);
      cacheSet(key, dataVersion, []);
      return;
    }

    // Incremental SWR fan-out. We still fetch every calendar in
    // parallel (the "revalidate" half of SWR), but we fold each
    // calendar's result in AS IT LANDS instead of awaiting the whole
    // batch. A slow external calendar — e.g. an EWS cold-path fetch
    // that takes seconds — therefore no longer blocks the fast local /
    // iCal calendars from painting.
    //
    // Progressive painting only happens on a COLD start (nothing
    // cached). On a cache hit the cached batch stays on screen until
    // the final authoritative swap below, so the view never briefly
    // shrinks to a partial set while calendars trickle in.
    const rangeStart = new Date(startIso);
    const rangeEnd = new Date(endIso);
    const perCalendar = new Map<string, CalendarEvent[]>();
    let remaining = ids.length;

    // Expand recurring masters into individual occurrences in-range.
    // The backend stores one master row per recurring event (+ its
    // RRULE); rrule.js expands in the browser. Re-run on each arrival
    // over the accumulated set — cheap for the handful of calendars in
    // play.
    const aggregate = (): CalendarEvent[] =>
      expandAll(Array.from(perCalendar.values()).flat(), {
        start: rangeStart,
        end: rangeEnd,
      });

    ids.forEach((id) => {
      getEvents({ calendar_id: id, start: startIso, end: endIso })
        .then(
          (batch) => batch,
          (err) => {
            // Keep the other calendars' data when one fails — better a
            // partial view than a blank screen.
            // eslint-disable-next-line no-console
            console.warn('get_events failed for calendar', id, err);
            return [] as CalendarEvent[];
          },
        )
        .then((batch) => {
          if (cancelled) return;
          perCalendar.set(id, batch);
          remaining -= 1;
          if (remaining === 0) {
            // Last calendar in: authoritative swap + cache write.
            const expanded = aggregate();
            cacheSet(key, dataVersion, expanded);
            setEvents(expanded);
            setLoading(false);
          } else if (!hadCache) {
            // Cold start: paint what we have so far.
            setEvents(aggregate());
          }
        })
        .catch((err) => {
          if (cancelled) return;
          setError(err);
          setLoading(false);
        });
    });

    return () => {
      cancelled = true;
    };
    // Note: selectedCalendarIds is intentionally omitted — `idsKey`
    // is the stable string projection of its contents, and including
    // the Set ref would re-fetch on every CalendarStore update even
    // when no calendar actually changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storeLoading, key, dataVersion]);

  // Lookup table: calendar id → calendar object. Cheap, useful when a
  // view wants to colour-code or label events by source.
  const calendarById = useMemo(() => {
    const map = new Map<string, (typeof calendars)[number]>();
    calendars.forEach((c) => map.set(c.id, c));
    return map;
  }, [calendars]);

  return { events, loading, error, calendarById };
}
