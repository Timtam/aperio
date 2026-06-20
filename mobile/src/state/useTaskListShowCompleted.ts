import { useCallback, useEffect, useState } from 'react';

import { getUserPref, setUserPref } from '../api/prefs';

/**
 * Per-task-list "show completed tasks in calendar views" preference — the mobile
 * twin of the desktop hook (same `tasks.showCompletedInCalendar` user-pref key).
 *
 * The calendar day cells (Day/Week/Month, via CalendarDayList → filterTasksOnDay)
 * hide completed tasks by default. Some lists prefer the opposite — keep
 * checked-off items visible as a sense of progress. The dedicated Tasks screen
 * always shows everything; this only affects the calendar surfaces. Per-list and
 * host-LOCAL (a device-side user-pref).
 *
 * A SINGLE module-level shared store (not per-component state), persisting ONLY
 * on a user-initiated change — never on hydration. That avoids the write-back
 * race the desktop hook used to have (a freshly-mounted view writing its
 * hydrated copy back and clobbering a just-set value), and lets a toggle on the
 * Lists screen reach the calendar views immediately via the listener fan-out.
 */

const PREF_KEY = 'tasks.showCompletedInCalendar';

export type ShowCompletedMap = Record<string, boolean>;

let cache: ShowCompletedMap = {};
let loaded = false;
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const l of listeners) l();
}

function load(): Promise<void> {
  if (loaded) return Promise.resolve();
  if (loading) return loading;
  loading = getUserPref(PREF_KEY)
    .then((raw) => {
      if (raw) {
        try {
          const parsed = JSON.parse(raw) as ShowCompletedMap;
          if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
            cache = parsed;
          }
        } catch {
          // Bad JSON; keep the empty default.
        }
      }
    })
    .catch(() => {
      // Backend unreachable; empty map = hide everywhere. Do NOT persist here, so
      // a transient read failure can't wipe the saved prefs.
    })
    .finally(() => {
      loaded = true;
      loading = null;
      notify();
    });
  return loading;
}

// Persist the current cache. Called ONLY from a user action (setShow), never
// from hydration.
function persist(): void {
  void setUserPref(PREF_KEY, JSON.stringify(cache));
}

export interface TaskListShowCompleted {
  /** True when the list wants completed tasks kept in calendar views. */
  shouldShow: (listId: string) => boolean;
  /** Set the preference for one list and persist asynchronously. */
  setShow: (listId: string, value: boolean) => void;
  /** Flip the preference. */
  toggle: (listId: string) => void;
  /** True until the initial hydration round-trip returns. */
  hydrating: boolean;
}

export function useTaskListShowCompleted(): TaskListShowCompleted {
  // A per-consumer snapshot of the shared cache, refreshed with a FRESH ref on
  // every store change — so this consumer re-renders AND `shouldShow` gets a new
  // identity, letting memos keyed on it re-run (CalendarDayList's day buckets).
  const [snapshot, setSnapshot] = useState<ShowCompletedMap>(() => ({
    ...cache,
  }));
  useEffect(() => {
    const cb = () => setSnapshot({ ...cache });
    listeners.add(cb);
    void load();
    return () => {
      listeners.delete(cb);
    };
  }, []);

  const shouldShow = useCallback(
    (listId: string): boolean => snapshot[listId] === true,
    [snapshot],
  );

  const setShow = useCallback((listId: string, value: boolean): void => {
    const has = cache[listId] === true;
    if (value === has) return; // no-op
    if (value) {
      cache = { ...cache, [listId]: true };
    } else {
      // Only explicit `true` overrides persist (minimal-overrides idiom).
      const next = { ...cache };
      delete next[listId];
      cache = next;
    }
    persist();
    notify();
  }, []);

  const toggle = useCallback(
    (listId: string): void => {
      setShow(listId, cache[listId] !== true);
    },
    [setShow],
  );

  return { shouldShow, setShow, toggle, hydrating: !loaded };
}
