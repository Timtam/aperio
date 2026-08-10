import { useCallback, useEffect, useState } from 'react';

import { getUserPref } from '../api/client';
import { useDebouncedPrefWrite } from './useDebouncedPrefWrite';

/**
 * Width (in px) of the backlog column in the week / month planner, persisted
 * to the `user_prefs` table under `backlog.width` (so it travels with the
 * account like the other UI prefs). The user sets it by dragging the column's
 * right edge; the calendar grid beside it flexes to fill the rest.
 *
 * Reads hydrate on mount; writes debounce ~200ms so a drag doesn't hammer
 * SQLite on every pointer move. Values are clamped to a sane range on both
 * read and write.
 */

const PREF_KEY = 'backlog.width';
const WRITE_DEBOUNCE_MS = 200;

export const BACKLOG_WIDTH_DEFAULT = 224;
export const BACKLOG_WIDTH_MIN = 160;
export const BACKLOG_WIDTH_MAX = 560;

const clamp = (px: number): number =>
  Math.min(BACKLOG_WIDTH_MAX, Math.max(BACKLOG_WIDTH_MIN, Math.round(px)));

export interface BacklogWidth {
  /** Current width in px (clamped to [MIN, MAX]). */
  width: number;
  /** Set the width (clamped) and persist asynchronously. */
  setWidth: (px: number) => void;
  /** True until the initial hydration round-trip returns. */
  hydrating: boolean;
}

export function useBacklogWidth(): BacklogWidth {
  const [width, setWidthState] = useState(BACKLOG_WIDTH_DEFAULT);
  const [hydrating, setHydrating] = useState(true);

  // Hydrate from the user-prefs store once on mount.
  useEffect(() => {
    let cancelled = false;
    getUserPref(PREF_KEY)
      .then((raw) => {
        if (cancelled || !raw) return;
        const parsed = Number.parseInt(raw, 10);
        if (Number.isFinite(parsed)) setWidthState(clamp(parsed));
      })
      .catch(() => {
        // Backend unreachable during init; keep the default.
      })
      .finally(() => {
        if (!cancelled) setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Debounced persistence — and only on a real change. `backlog.width` syncs,
  // so a write-back of the value hydration just read would be a launch echo
  // able to overrule a width set on another device (see
  // `useDebouncedPrefWrite`).
  useDebouncedPrefWrite(PREF_KEY, String(width), hydrating, WRITE_DEBOUNCE_MS);

  const setWidth = useCallback((px: number) => {
    setWidthState(clamp(px));
  }, []);

  return { width, setWidth, hydrating };
}
