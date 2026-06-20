import { useCallback, useEffect, useState } from 'react';

import { getUserPref, setUserPref } from '../api/client';

/**
 * Per-task-list "show completed tasks in calendar views" preference.
 *
 * Calendar surfaces (WeekView, DayView, MonthView) hide completed tasks by
 * default — once a row is checked off, the user usually wants it out of the way
 * so the day grid doesn't fill up with done items. Some users prefer the
 * opposite: keep checked-off items visible (line-through styling) as a sense of
 * progress.
 *
 * The preference is per-list because tasks from different sources have different
 * cadences — a daily-grind work list might want completed items hidden, a hobby
 * list might want them kept. The dedicated Aufgaben view (TaskView) always shows
 * everything, so this setting only affects the calendar surfaces.
 *
 * Storage: a single JSON object under the `tasks.showCompletedInCalendar` key in
 * `user_prefs`. Default for missing keys: `false` (hide). Only explicit `true`
 * values persist — same minimal-overrides idiom as `useSidebarExpansion`.
 *
 * Architecture: a SINGLE module-level store shared by every consumer (the
 * Sidebar context menu + all calendar views), not per-component state. This is
 * load-bearing for correctness:
 *  - The store hydrates from `user_prefs` exactly once, and persists ONLY on a
 *    user-initiated change (setShow/toggle) — never on hydration. An earlier
 *    per-component version wrote the hydrated map back whenever `hydrating`
 *    flipped, so each of the four instances (Sidebar + 3 views) raced to write
 *    its own copy on every mount; a stale or failed read then clobbered a
 *    just-toggled value. Persisting only real edits removes that whole class of
 *    races.
 *  - A shared store also means a toggle in the Sidebar updates the calendar
 *    views immediately (one cache + a listener fan-out), instead of leaving each
 *    view on its own stale copy until it remounts.
 */

const PREF_KEY = 'tasks.showCompletedInCalendar';

export type ShowCompletedMap = Record<string, boolean>;

// Shared store state. `cache` is the single source of truth; `loaded` guards a
// one-shot hydration; `loading` dedupes concurrent first-mount loads.
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
          // Bad JSON; keep the empty default so the user isn't stuck.
        }
      }
    })
    .catch(() => {
      // Backend unreachable during init; empty map = hide everywhere. Crucially
      // we do NOT persist here, so a transient read failure can't wipe the saved
      // prefs.
    })
    .finally(() => {
      loaded = true;
      loading = null;
      notify();
    });
  return loading;
}

// Persist the current cache. Called ONLY from a user action (setShow), never
// from hydration — so a failed/empty read can never clobber the saved prefs.
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
  // identity, letting memos keyed on it re-run (e.g. a calendar view's day
  // buckets). A fresh ref every time (even when the cache object is unchanged)
  // also guarantees the post-hydration render fires.
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
    if (value === has) return; // no-op; don't churn writes
    if (value) {
      cache = { ...cache, [listId]: true };
    } else {
      // Default is "hide" — only explicit `true` overrides persist, mirroring
      // how `useSidebarExpansion` keeps its JSON minimal.
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
