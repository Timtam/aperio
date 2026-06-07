import { useCallback, useEffect, useRef, useState } from 'react';

import { getUserPref, setUserPref } from '../api/client';

/**
 * Whole-sidebar collapsed state — a single boolean, persisted to the
 * `user_prefs` table under `sidebar.collapsed`. Distinct from
 * {@link useSidebarExpansion}, which tracks per-node *tree* expansion; this is
 * "is the entire sidebar hidden". Default (missing key) is expanded
 * (`collapsed = false`). Reads hydrate on mount; writes debounce ~150ms so a
 * rapid toggle doesn't hammer SQLite.
 */

const PREF_KEY = 'sidebar.collapsed';
const WRITE_DEBOUNCE_MS = 150;

export interface SidebarCollapse {
  /** True when the sidebar is hidden. */
  collapsed: boolean;
  /** Set the collapsed state and persist asynchronously. */
  setCollapsed: (value: boolean) => void;
  /** Flip the collapsed state. */
  toggle: () => void;
  /** True until the initial hydration round-trip returns. */
  hydrating: boolean;
}

export function useSidebarCollapse(): SidebarCollapse {
  const [collapsed, setCollapsedState] = useState(false);
  const [hydrating, setHydrating] = useState(true);

  // Hydrate from the user-prefs store once on mount.
  useEffect(() => {
    let cancelled = false;
    getUserPref(PREF_KEY)
      .then((raw) => {
        if (cancelled) return;
        if (raw === 'true') setCollapsedState(true);
      })
      .catch(() => {
        // Backend unreachable during init; default expanded.
      })
      .finally(() => {
        if (!cancelled) setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Debounced persistence.
  const writeTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (writeTimer.current !== null) {
      window.clearTimeout(writeTimer.current);
    }
    writeTimer.current = window.setTimeout(() => {
      void setUserPref(PREF_KEY, collapsed ? 'true' : 'false');
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (writeTimer.current !== null) {
        window.clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
    };
  }, [collapsed, hydrating]);

  const setCollapsed = useCallback((value: boolean) => {
    setCollapsedState(value);
  }, []);

  const toggle = useCallback(() => {
    setCollapsedState((c) => !c);
  }, []);

  return { collapsed, setCollapsed, toggle, hydrating };
}
