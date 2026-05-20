import { useCallback, useEffect, useRef, useState } from 'react';

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
 * The store treats *missing* keys as "expanded" — i.e. the default
 * state when a user first sees the tree is everything open. Only
 * explicit collapses produce entries in the persisted map. That keeps
 * the JSON small and makes "reset to defaults" a one-liner (drop the
 * whole pref).
 *
 * Persistence: the value is a JSON object mapping key → boolean. We
 * round-trip it through the `user_prefs` table via the
 * `sidebar.expansion` key. Reads hydrate on mount; writes debounce
 * by ~150ms so rapid Up/Down keyboard collapse doesn't hammer SQLite.
 */

const PREF_KEY = 'sidebar.expansion';
const WRITE_DEBOUNCE_MS = 150;

export type ExpansionMap = Record<string, boolean>;

export interface SidebarExpansion {
  /** True when the node is open. Default for missing keys: `true`. */
  isExpanded: (nodeKey: string) => boolean;
  /** Set expansion state for one node and persist asynchronously. */
  setExpanded: (nodeKey: string, value: boolean) => void;
  /** Toggle expansion state for one node. */
  toggle: (nodeKey: string) => void;
  /** True until the initial hydration round-trip returns. Components
   *  can use it to avoid flashing the wrong initial state. */
  hydrating: boolean;
}

export function useSidebarExpansion(): SidebarExpansion {
  const [map, setMap] = useState<ExpansionMap>({});
  const [hydrating, setHydrating] = useState(true);

  // Hydrate from the user-prefs store once on mount.
  useEffect(() => {
    let cancelled = false;
    getUserPref(PREF_KEY)
      .then((raw) => {
        if (cancelled) return;
        if (raw) {
          try {
            const parsed = JSON.parse(raw) as ExpansionMap;
            // Defensive type-narrow: any non-object input → reset to
            // an empty map (= everything expanded).
            if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
              setMap(parsed);
            }
          } catch {
            // Bad JSON; treat as empty so the user isn't stuck.
          }
        }
      })
      .catch(() => {
        // Backend unreachable (e.g. during the initial DB-open race);
        // empty map = everything expanded. Same as a fresh install.
      })
      .finally(() => {
        if (!cancelled) setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Persist debounced. The ref-based timer prevents stacking; the
  // closure-captured `map` is always the latest because the effect
  // depends on it.
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

  const isExpanded = useCallback(
    (nodeKey: string): boolean => {
      // Missing → expanded. The map only ever stores explicit overrides,
      // which is almost always `false` (the user collapsed this node).
      const v = map[nodeKey];
      return v === undefined ? true : v;
    },
    [map],
  );

  const setExpanded = useCallback((nodeKey: string, value: boolean) => {
    setMap((prev) => {
      // If the value matches the default (true / undefined) we drop
      // the key so the stored JSON stays minimal — only divergences
      // from the "everything expanded" default need to persist.
      if (value) {
        if (prev[nodeKey] === undefined) return prev;
        const next = { ...prev };
        delete next[nodeKey];
        return next;
      }
      if (prev[nodeKey] === false) return prev;
      return { ...prev, [nodeKey]: false };
    });
  }, []);

  const toggle = useCallback(
    (nodeKey: string) => {
      setExpanded(nodeKey, !isExpanded(nodeKey));
    },
    [isExpanded, setExpanded],
  );

  return { isExpanded, setExpanded, toggle, hydrating };
}
