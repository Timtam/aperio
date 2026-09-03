import { useEffect, useMemo, useState } from 'react';

import { getEvents } from '../api/client';
import type { CalendarEvent } from '../api/types';
import { expandAll } from '../intl/recurrence';
import { useCalendarStore } from './calendarStoreContext';
import { useDialogState } from './dialogStateContext';
import { useViewState } from './viewStateContext';

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
 *   `dataVersion` is the global "data may have changed" counter (bumped
 *   after every dialog close, explicit `invalidateData()`, and each
 *   background `cache-updated`). It's an effect *dependency*, so a bump
 *   triggers a revalidation — but the cached entry is KEPT and served as
 *   stale meanwhile, so the view never blanks or shrinks to a partial
 *   set while the refetch runs. The authoritative swap below overwrites
 *   the entry when the fresh batch lands.
 *
 *   (Wiping the cache wholesale on every `dataVersion` bump — as an
 *   earlier version did — turned each background-refresh-triggered
 *   refetch into a COLD start: the progressive per-calendar paint would
 *   shrink the list back to one calendar and grow it again, which both
 *   flickered visually and made screen readers re-announce the day's
 *   event count on every step. Keeping the entry is the actual SWR
 *   contract and avoids that churn.)
 */

/** Cache key: sorted calendar IDs joined with ` ` + range ISO strings. */
type CacheKey = string;

const eventsCache = new Map<CacheKey, CalendarEvent[]>();

/**
 * Last successful raw batch per `(calendar, range)` — the monotonic
 * per-container layer under the aggregate cache above. Every fan-out
 * seeds from it and updates it per calendar, so
 *
 *   - a re-triggered or re-keyed run starts from the union of every
 *     calendar's LAST KNOWN batch instead of from empty (the cold-start
 *     progressive paint used to collapse the day to whichever calendar
 *     answered first and re-grow it — the app-start count oscillation), and
 *   - a calendar whose fetch FAILS keeps its previous batch on screen
 *     instead of shrinking the aggregate to a partial set.
 *
 * Genuine removals still propagate: a deselected calendar simply isn't
 * part of the fan-out (its slice is dropped from the run's map), and a
 * successful fetch replaces the calendar's slice verbatim — including
 * with an empty batch when the provider really has nothing.
 */
const perCalendarCache = new Map<string, CalendarEvent[]>();

function perCalendarKey(id: string, startIso: string, endIso: string): string {
  return `${id}|${startIso}|${endIso}`;
}

function cacheKey(idsKey: string, startIso: string, endIso: string): CacheKey {
  return `${idsKey}|${startIso}|${endIso}`;
}

function cacheGet(key: CacheKey): CalendarEvent[] | undefined {
  return eventsCache.get(key);
}

function cacheSet(key: CacheKey, events: CalendarEvent[]): void {
  eventsCache.set(key, events);
}

/** The hook state a key starts from: the cached batch (loaded), or empty
 *  and loading. Pure cache read — safe to call during render. */
function seedFromCache(key: CacheKey): {
  key: CacheKey;
  events: CalendarEvent[];
  loading: boolean;
} {
  const cached = cacheGet(key);
  return { key, events: cached ?? [], loading: cached === undefined };
}

/** Test-only escape hatch — wipes the caches so each test starts clean. */
export function __resetEventsCacheForTests(): void {
  eventsCache.clear();
  perCalendarCache.clear();
}

export function useEvents(range: { start: Date; end: Date }) {
  const { selectedCalendarIds, calendars, calendarsLoading } =
    useCalendarStore();
  const { dataVersion } = useDialogState();
  const { focusedCalendarId, showCancelledEvents } = useViewState();

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

  // The state is TAGGED with the key it describes, and a render whose key
  // differs derives its value from the cache right here instead of showing
  // the tagged state. The effect below re-seeds the state on a key change,
  // but effects run AFTER the render — so the one render in between used to
  // show the PREVIOUS range's events under the NEW range's days: paging a
  // week, every day cell first announced the count of whatever old-range
  // events happened to span into it (a multi-day holiday: "1 Eintrag"), then
  // re-announced the real count a frame later. That flash happened on a
  // cache HIT too, which read, from the outside, as if paging back had thrown
  // the cache away. A cold key derives to empty+loading, never to stale rows.
  const [state, setState] = useState<{
    key: CacheKey;
    events: CalendarEvent[];
    loading: boolean;
  }>(() => seedFromCache(key));
  const current = state.key === key ? state : seedFromCache(key);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);

    // Cache check on every (re)run. If the key changed or the
    // dataVersion ticked, this is where state snaps to the cached
    // batch (cache hit, loading stays / flips false) or arms a real
    // fetch (cache miss).
    const cached = cacheGet(key);
    const hadCache = cached !== undefined;
    // Functional updates that hand back `prev` when nothing changed keep
    // React's bail-out: a dataVersion bump on a warm key must not cost every
    // consumer view an extra render.
    if (cached) {
      setState((prev) =>
        prev.key === key && prev.events === cached && !prev.loading
          ? prev
          : { key, events: cached, loading: false },
      );
    } else {
      setState((prev) =>
        prev.key === key
          ? prev.loading
            ? prev
            : { ...prev, loading: true }
          : { key, events: [], loading: true },
      );
    }

    // Defer the actual network call until the CALENDAR catalog is ready
    // (not the whole store) — events only need calendars; gating on the
    // aggregate would make a slow task-list or contacts source delay the
    // calendar view too. The cache short-circuit above already runs
    // unconditionally — having stale events on screen is fine while the
    // store still settles.
    if (calendarsLoading) return;

    const ids = effectiveIds;
    if (ids.length === 0) {
      setState({ key, events: [], loading: false });
      cacheSet(key, []);
      return;
    }

    // Incremental SWR fan-out. We still fetch every calendar in
    // parallel (the "revalidate" half of SWR), but we fold each
    // calendar's result in AS IT LANDS instead of awaiting the whole
    // batch. A slow external calendar — e.g. an EWS cold-path fetch
    // that takes seconds — therefore no longer blocks the fast local /
    // iCal calendars from painting.
    //
    // The run's map is SEEDED from the per-calendar cache, so a cold-key
    // run (first paint, selection change, re-trigger mid-flight) starts
    // from every calendar's last known batch and each arrival only
    // replaces its own calendar's slice — the aggregate count can't
    // collapse to the first responder and re-grow. On a cache hit the
    // cached batch additionally stays on screen untouched until the
    // final authoritative swap.
    const rangeStart = new Date(startIso);
    const rangeEnd = new Date(endIso);
    const perCalendar = new Map<string, CalendarEvent[]>();
    ids.forEach((id) => {
      const prev = perCalendarCache.get(perCalendarKey(id, startIso, endIso));
      if (prev) perCalendar.set(id, prev);
    });
    let remaining = ids.length;

    // Expand recurring masters into individual occurrences in-range.
    // The backend stores one master row per recurring event (+ its
    // RRULE); rrule.js expands in the browser. Re-run on each arrival
    // over the accumulated set — cheap for the handful of calendars in
    // play.
    //
    // Everything a calendar holds, INCLUDING the meetings a videoconference
    // account contributes for appointments that already have a calendar entry.
    // Hiding those duplicates is `useEventGroups`' job now, because the honest
    // way to hide one is to group the two — and pairing them needs both rows,
    // which only exist before any filtering. See `DESIGN-event-groups.md`,
    // Stufe 4.
    const aggregate = (): CalendarEvent[] =>
      expandAll(Array.from(perCalendar.values()).flat(), {
        start: rangeStart,
        end: rangeEnd,
      });

    ids.forEach((id) => {
      const ckey = perCalendarKey(id, startIso, endIso);
      getEvents({ calendar_id: id, start: startIso, end: endIso })
        .then(
          (batch) => {
            // Fence the retention write on the run being current (the
            // useTasks fence, same rationale): a superseded run's slow
            // response landing AFTER the newer run's fresh write would
            // put PRE-mutation data back into the cache — a later
            // cold-key seed or failure fallback would then resurrect
            // e.g. a just-deleted event.
            if (!cancelled) perCalendarCache.set(ckey, batch);
            return batch;
          },
          (err) => {
            // A transient failure keeps the calendar's LAST KNOWN batch
            // (seeded above) — one hiccuping backend must not shrink the
            // visible day. Only a calendar that never answered in this
            // session degrades to empty.
            // eslint-disable-next-line no-console
            console.warn('get_events failed for calendar', id, err);
            return perCalendarCache.get(ckey) ?? ([] as CalendarEvent[]);
          },
        )
        .then((batch) => {
          if (cancelled) return;
          perCalendar.set(id, batch);
          remaining -= 1;
          if (remaining === 0) {
            // Last calendar in: authoritative swap. The aggregate is
            // cached even when a container failed — it IS what is being
            // displayed (failure-holes are patched from each container's
            // last known batch), and it is strictly newer than whatever
            // older entry the key held. Freezing the old entry instead
            // made every later dataVersion bump repaint outdated data
            // first (grow-shrink flicker) for as long as one container
            // kept failing.
            const expanded = aggregate();
            cacheSet(key, expanded);
            setState({ key, events: expanded, loading: false });
          } else if (!hadCache) {
            // Cold key: paint what we have so far (last-known union with
            // this calendar's slice refreshed). Still loading — the
            // authoritative swap above flips that.
            setState({ key, events: aggregate(), loading: true });
          }
        })
        .catch((err) => {
          if (cancelled) return;
          setError(err);
          setState((prev) =>
            prev.key === key
              ? { ...prev, loading: false }
              : { key, events: [], loading: false },
          );
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
  }, [calendarsLoading, key, dataVersion]);

  // Lookup table: calendar id → calendar object. Cheap, useful when a
  // view wants to colour-code or label events by source.
  const calendarById = useMemo(() => {
    const map = new Map<string, (typeof calendars)[number]>();
    calendars.forEach((c) => map.set(c.id, c));
    return map;
  }, [calendars]);

  // Cancelled events are cached raw; the show-cancelled toggle filters them at
  // read time so flipping it re-filters instantly without a refetch. (Reminders
  // for cancelled events are suppressed separately, core-side, regardless.)
  const events = current.events;
  const visibleEvents = useMemo(
    () =>
      showCancelledEvents ? events : events.filter((e) => !e.cancelled),
    [events, showCancelledEvents],
  );

  return { events: visibleEvents, loading: current.loading, error, calendarById };
}
