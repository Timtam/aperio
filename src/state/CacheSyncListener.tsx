import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';

import { getCacheRefreshStatus } from '../api/client';
import { useCalendarStore } from './calendarStoreContext';
import { registerDataReloadSink } from './dataReloadBus';
import { useDialogState } from './dialogStateContext';

/**
 * Bridges the backend `cache-updated` push (CACHE-1/2 stale-while-
 * revalidate) to the frontend's data layer.
 *
 * When the host serves an external read from its snapshot cache it kicks
 * a background refresh; once that lands it writes the fresh data and
 * emits `cache-updated` with the affected `scope`. We coalesce a burst
 * (app start warms many containers at once) and then, per scope:
 *
 *   - item scopes (`events` / `tasks` / `contacts`) → bump `dataVersion`
 *     so the SWR hooks (`useEvents` / `useTasks` / `useContacts`) refetch
 *     and pick up the now-fresh cache;
 *   - listing scopes (`calendars` / `task_lists` / `contact_lists`) →
 *     re-run the matching CalendarStore refresh so the sidebar updates.
 *
 * Reload-wave gating: a warm pass spreads its per-container emissions
 * over seconds, which used to defeat a fixed trailing debounce and turn
 * one pass into MANY full refetch waves (each re-exposing whatever
 * intermediate cache state existed — the app-start day-count
 * oscillation). The listener therefore tracks `cache-refresh-status`:
 * while a pass is in flight, flushes are throttled to one per
 * [`PASS_THROTTLE_MS`]; the pass-end status flushes immediately, so the
 * settled state paints exactly once. A hung pass can't starve the UI —
 * the throttle still flushes on its own cadence.
 *
 * Other data-changed producers (the sync scheduler's round-end) feed the
 * SAME coalescer via `nudgeDataReload` (dataReloadBus.ts) instead of
 * bumping `dataVersion` synchronously, so a sync round landing mid-pass
 * no longer bypasses the gating.
 *
 * Renders nothing.
 */
interface CacheUpdatedPayload {
  scope: string;
  account_id: string;
  container_id: string;
}

interface CacheRefreshStatusPayload {
  refreshing: boolean;
}

/** Coalesce window for a burst of cache-updated events (no pass running). */
const COALESCE_MS = 250;

/** Max flush cadence while a backend warm pass is in flight. */
const PASS_THROTTLE_MS = 2500;

export function CacheSyncListener(): null {
  const { invalidateData } = useDialogState();
  const { refreshCalendars, refreshTaskLists, refreshContactLists } =
    useCalendarStore();

  // Keep the latest callbacks in a ref so the listen() effect can stay
  // mounted once for the app's lifetime without re-subscribing.
  const handlers = useRef({
    invalidateData,
    refreshCalendars,
    refreshTaskLists,
    refreshContactLists,
  });
  handlers.current = {
    invalidateData,
    refreshCalendars,
    refreshTaskLists,
    refreshContactLists,
  };

  useEffect(() => {
    const unlistens: Array<() => void> = [];
    let timer: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;
    let refreshing = false;
    // Pending scopes accumulated within the current coalesce window.
    const pending = new Set<string>();

    // Seed the pass state: a listener mounting MID-pass (webview reload,
    // renderer restart) would otherwise coalesce the pass's remaining
    // emissions at the short window until the next status event happens to
    // arrive — the multi-wave behavior the throttle exists to prevent.
    getCacheRefreshStatus()
      .then((s) => {
        if (!disposed) refreshing = s.refreshing;
      })
      .catch((err) => {
        console.warn('get_cache_refresh_status failed', err);
      });

    const flush = () => {
      timer = undefined;
      if (pending.size === 0) return;
      const scopes = new Set(pending);
      pending.clear();
      const h = handlers.current;
      const listingChanged =
        scopes.has('calendars') ||
        scopes.has('task_lists') ||
        scopes.has('contact_lists');
      if (scopes.has('calendars')) void h.refreshCalendars();
      if (scopes.has('task_lists')) void h.refreshTaskLists();
      if (scopes.has('contact_lists')) void h.refreshContactLists();
      // Item-scope changes invalidate the SWR data hooks. A *listing*-scope
      // change does too: when a cold catalog finishes its background refresh
      // it registers the container→account routes for the first time, so the
      // item hooks must re-fetch to pick up external events/tasks/contacts
      // that couldn't be routed (and so came back empty) on the cold first
      // paint. Without this the sidebar would fill but the view would stay
      // blank until an unrelated refresh nudged it.
      if (
        listingChanged ||
        scopes.has('events') ||
        scopes.has('tasks') ||
        scopes.has('contacts')
      ) {
        h.invalidateData();
      }
    };

    // Collect-then-flush: the FIRST event of a window arms the timer;
    // later events just accumulate (no per-event reset, so latency is
    // bounded and a drip of events can't postpone the flush forever).
    const schedule = () => {
      if (timer !== undefined) return;
      timer = setTimeout(flush, refreshing ? PASS_THROTTLE_MS : COALESCE_MS);
    };

    registerDataReloadSink((scopes) => {
      scopes.forEach((s) => pending.add(s));
      schedule();
    });

    listen<CacheUpdatedPayload>('cache-updated', (event) => {
      pending.add(event.payload.scope);
      schedule();
    })
      .then((fn) => {
        if (disposed) fn();
        else unlistens.push(fn);
      })
      .catch((err) => {
        console.warn('cache-updated listen failed', err);
      });

    listen<CacheRefreshStatusPayload>('cache-refresh-status', (event) => {
      const was = refreshing;
      refreshing = event.payload.refreshing;
      if (was && !refreshing) {
        // Pass end: paint the settled state NOW instead of waiting out a
        // long throttle window armed mid-pass.
        if (timer) clearTimeout(timer);
        timer = undefined;
        flush();
      }
    })
      .then((fn) => {
        if (disposed) fn();
        else unlistens.push(fn);
      })
      .catch((err) => {
        console.warn('cache-refresh-status listen failed', err);
      });

    return () => {
      disposed = true;
      registerDataReloadSink(null);
      if (timer) clearTimeout(timer);
      unlistens.forEach((fn) => fn());
    };
  }, []);

  return null;
}
