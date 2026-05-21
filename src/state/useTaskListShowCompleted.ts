import { useCallback, useEffect, useRef, useState } from 'react';

import { getUserPref, setUserPref } from '../api/client';

/**
 * Per-task-list "show completed tasks in calendar views" preference.
 *
 * Calendar surfaces (WeekView, DayView, MonthView once it grows tasks)
 * hide completed tasks by default — once a row is checked off, the
 * user usually wants it out of the way so the day grid doesn't fill
 * up with done items. Some users prefer the opposite: keep checked-off
 * items visible (line-through styling) as a sense of progress.
 *
 * The preference is per-list because tasks from different sources
 * have different cadences — a daily-grind work list might want
 * completed items hidden, a hobby list might want them kept. The
 * dedicated Aufgaben view (TaskView) always shows everything, so this
 * setting only affects the calendar surfaces.
 *
 * Storage: a single JSON object under the `tasks.showCompletedInCalendar`
 * key in `user_prefs`. Default for missing keys: `false` (hide). Only
 * explicit `true` values persist — same minimal-overrides idiom as
 * `useSidebarExpansion`.
 */

const PREF_KEY = 'tasks.showCompletedInCalendar';
const WRITE_DEBOUNCE_MS = 150;

export type ShowCompletedMap = Record<string, boolean>;

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
  const [map, setMap] = useState<ShowCompletedMap>({});
  const [hydrating, setHydrating] = useState(true);

  // Hydrate from the user-prefs store once on mount.
  useEffect(() => {
    let cancelled = false;
    getUserPref(PREF_KEY)
      .then((raw) => {
        if (cancelled) return;
        if (raw) {
          try {
            const parsed = JSON.parse(raw) as ShowCompletedMap;
            if (
              parsed &&
              typeof parsed === 'object' &&
              !Array.isArray(parsed)
            ) {
              setMap(parsed);
            }
          } catch {
            // Bad JSON; default to empty so the user isn't stuck.
          }
        }
      })
      .catch(() => {
        // Backend unreachable during init; empty map = hide everywhere.
      })
      .finally(() => {
        if (!cancelled) setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Debounced persistence — toggling repeatedly via the context menu
  // shouldn't hammer SQLite.
  const writeTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (writeTimer.current !== null) {
      window.clearTimeout(writeTimer.current);
    }
    writeTimer.current = window.setTimeout(() => {
      void setUserPref(PREF_KEY, JSON.stringify(map));
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (writeTimer.current !== null) {
        window.clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
    };
  }, [map, hydrating]);

  const shouldShow = useCallback(
    (listId: string): boolean => map[listId] === true,
    [map],
  );

  const setShow = useCallback((listId: string, value: boolean) => {
    setMap((prev) => {
      // Default is "hide" — only explicit `true` overrides persist,
      // mirroring how `useSidebarExpansion` keeps its JSON minimal.
      if (!value) {
        if (prev[listId] === undefined) return prev;
        const next = { ...prev };
        delete next[listId];
        return next;
      }
      if (prev[listId] === true) return prev;
      return { ...prev, [listId]: true };
    });
  }, []);

  const toggle = useCallback(
    (listId: string) => {
      setShow(listId, !shouldShow(listId));
    },
    [shouldShow, setShow],
  );

  return { shouldShow, setShow, toggle, hydrating };
}
