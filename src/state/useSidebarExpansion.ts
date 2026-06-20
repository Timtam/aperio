import { useCallback, useEffect, useState } from 'react';

import { getUserPref, setUserPref } from '../api/client';

/**
 * Sidebar tree expansion state.
 *
 * Each node key encodes its position in the tree:
 *
 *   `account:{accountId}`              — top-level account node
 *   `account:{accountId}#calendars`    — "Calendars" sub-section
 *   `account:{accountId}#tasks`        — "Tasks" sub-section
 *
 * The store treats *missing* keys as "expanded" — i.e. the default state when a
 * user first sees the tree is everything open. Only explicit collapses produce
 * entries in the persisted map. That keeps the JSON small and makes "reset to
 * defaults" a one-liner (drop the whole pref).
 *
 * Persistence: a JSON object mapping key → boolean, round-tripped through the
 * `user_prefs` table under the `sidebar.expansion` key. Writes debounce by
 * ~150ms so rapid Up/Down keyboard collapse doesn't hammer SQLite.
 *
 * Architecture: a SINGLE module-level shared store, hydrated once and persisted
 * ONLY on a user-initiated change (setExpanded/toggle) — never on hydration. An
 * earlier per-component version wrote its hydrated map back as soon as
 * `hydrating` flipped, so a transient empty/failed read during init would
 * persist `{}` and wipe every saved collapse. Persisting only real edits removes
 * that clobber; a shared cache + listener fan-out also keeps any future second
 * consumer in sync. (Mirrors the fix in `useTaskListShowCompleted`.)
 */

const PREF_KEY = 'sidebar.expansion';
const WRITE_DEBOUNCE_MS = 150;

export type ExpansionMap = Record<string, boolean>;

// Shared store state. `cache` is the single source of truth; `loaded` guards a
// one-shot hydration; `loading` dedupes concurrent first-mount loads.
let cache: ExpansionMap = {};
let loaded = false;
let loading: Promise<void> | null = null;
let writeTimer: ReturnType<typeof setTimeout> | null = null;
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
          const parsed = JSON.parse(raw) as ExpansionMap;
          // Defensive type-narrow: any non-object input → empty map (=
          // everything expanded).
          if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
            cache = parsed;
          }
        } catch {
          // Bad JSON; treat as empty so the user isn't stuck.
        }
      }
    })
    .catch(() => {
      // Backend unreachable (e.g. the initial DB-open race); empty map =
      // everything expanded. Crucially we do NOT persist here, so a transient
      // read failure can't wipe the saved collapses.
    })
    .finally(() => {
      loaded = true;
      loading = null;
      notify();
    });
  return loading;
}

// Persist (debounced) the current cache. Called ONLY from a user action, never
// from hydration.
function persist(): void {
  if (writeTimer !== null) clearTimeout(writeTimer);
  writeTimer = setTimeout(() => {
    writeTimer = null;
    void setUserPref(PREF_KEY, JSON.stringify(cache));
  }, WRITE_DEBOUNCE_MS);
}

export interface SidebarExpansion {
  /** True when the node is open. Default for missing keys: `true`. */
  isExpanded: (nodeKey: string) => boolean;
  /** Set expansion state for one node and persist asynchronously. */
  setExpanded: (nodeKey: string, value: boolean) => void;
  /** Toggle expansion state for one node. */
  toggle: (nodeKey: string) => void;
  /** True until the initial hydration round-trip returns. Components can use it
   *  to avoid flashing the wrong initial state. */
  hydrating: boolean;
}

export function useSidebarExpansion(): SidebarExpansion {
  // A per-consumer snapshot of the shared cache, refreshed with a FRESH ref on
  // every store change — so this consumer re-renders AND `isExpanded` gets a new
  // identity, letting memos keyed on it re-run. A fresh ref every time also
  // guarantees the post-hydration render fires.
  const [snapshot, setSnapshot] = useState<ExpansionMap>(() => ({ ...cache }));
  useEffect(() => {
    const cb = () => setSnapshot({ ...cache });
    listeners.add(cb);
    void load();
    return () => {
      listeners.delete(cb);
    };
  }, []);

  const isExpanded = useCallback(
    (nodeKey: string): boolean => {
      // Missing → expanded. The map only ever stores explicit overrides, which
      // are almost always `false` (the user collapsed this node).
      const v = snapshot[nodeKey];
      return v === undefined ? true : v;
    },
    [snapshot],
  );

  const setExpanded = useCallback((nodeKey: string, value: boolean): void => {
    // If the value matches the default (true / undefined) we drop the key so the
    // stored JSON stays minimal — only divergences from the "everything
    // expanded" default need to persist. Read/write the shared cache directly
    // (the source of truth), not the render snapshot.
    if (value) {
      if (cache[nodeKey] === undefined) return;
      const next = { ...cache };
      delete next[nodeKey];
      cache = next;
    } else {
      if (cache[nodeKey] === false) return;
      cache = { ...cache, [nodeKey]: false };
    }
    persist();
    notify();
  }, []);

  const toggle = useCallback(
    (nodeKey: string): void => {
      const current = cache[nodeKey];
      const expanded = current === undefined ? true : current;
      setExpanded(nodeKey, !expanded);
    },
    [setExpanded],
  );

  return { isExpanded, setExpanded, toggle, hydrating: !loaded };
}
