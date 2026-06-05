import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';

import { useCalendarStore } from './calendarStoreContext';
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
 * Renders nothing.
 */
interface CacheUpdatedPayload {
  scope: string;
  account_id: string;
  container_id: string;
}

/** Debounce window for coalescing a startup burst of cache-updated events. */
const COALESCE_MS = 250;

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
    let unlisten: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;
    // Pending scopes accumulated within the current coalesce window.
    const pending = new Set<string>();

    const flush = () => {
      timer = undefined;
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

    listen<CacheUpdatedPayload>('cache-updated', (event) => {
      pending.add(event.payload.scope);
      if (timer) clearTimeout(timer);
      timer = setTimeout(flush, COALESCE_MS);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((err) => {
        console.warn('cache-updated listen failed', err);
      });

    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
      unlisten?.();
    };
  }, []);

  return null;
}
