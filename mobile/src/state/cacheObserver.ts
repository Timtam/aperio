// External-cache live-update wiring. The Rust Host pushes a `cache_updated`
// callback whenever a background refresh / warm pass writes fresh external data;
// the native module forwards it as the Expo `onCacheUpdated` event. Here we
// coalesce those per-container events over a short window, then — per the user's
// choice (live-update WITH a polite announcement, not a silent swap) — announce
// the refresh and notify the focused screen to reload.
//
// Screen-reader-first: the announcement is a POLITE
// `announceForAccessibility` (it doesn't steal focus), and the reload is
// coalesced so a warm pass touching many containers speaks once per category,
// not once per calendar.

import { useEffect, useRef } from 'react';
import { AccessibilityInfo } from 'react-native';
import { useTranslation } from 'react-i18next';

import type { CacheRefreshStatus } from '../api/sync';
import { setCacheRefreshProgress } from './cacheRefreshProgress';
import { hapticLoadBegin, hapticLoadEnd, loadHapticsPref } from './haptics';
import CalFfi from '../../modules/cal-ffi';

/** Coarse category a cache scope belongs to — drives the announcement string +
 *  which screens reload. The Host scopes are events/calendars (→ calendar),
 *  tasks/task_lists (→ tasks), contacts/contact_lists (→ contacts). */
export type CacheCategory = 'calendar' | 'tasks' | 'contacts';

function categoryForScope(scope: string): CacheCategory | null {
  switch (scope) {
    case 'events':
    case 'calendars':
      return 'calendar';
    case 'tasks':
    case 'task_lists':
      return 'tasks';
    case 'contacts':
    case 'contact_lists':
      return 'contacts';
    default:
      return null;
  }
}

// Module-level bus: the root observer fans coalesced category notifications out
// to whichever screens are subscribed (via useCacheReload).
type BusListener = (category: CacheCategory) => void;
const busListeners = new Set<BusListener>();

function subscribeBus(cb: BusListener): () => void {
  busListeners.add(cb);
  return () => {
    busListeners.delete(cb);
  };
}

/**
 * Fan an immediate reload to every screen subscribed via `useCacheReload`, across
 * ALL categories. Use after a CROSS-DEVICE sync round (or onboarding) applies a
 * peer's data: that path writes straight to the local store and never goes
 * through the Host's external `onCacheUpdated` push, so without this the open
 * screens stay stale until the app restarts. Silent — it's a data reload, not the
 * external-refresh cue.
 */
export function notifyDataReload(): void {
  const categories: CacheCategory[] = ['calendar', 'tasks', 'contacts'];
  for (const cat of categories) {
    busListeners.forEach((l) => l(cat));
  }
}

/** Coalesce window: a warm pass emits one event per container in a burst; wait
 *  this long after the last before announcing + reloading once per category. */
const COALESCE_MS = 700;

/**
 * Mount ONCE near the app root. Subscribes to the native external-cache push,
 * coalesces the burst, announces each refreshed category politely, and notifies
 * the bus so the focused view live-reloads.
 */
export function useCacheUpdates(): void {
  const { t } = useTranslation();
  const pending = useRef<Set<CacheCategory>>(new Set());
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Last-seen warm-pass state, so we announce only the START and END of a pass.
  const refreshing = useRef(false);

  // Prime the device-local haptics pref once (default on).
  useEffect(() => {
    void loadHapticsPref();
  }, []);

  useEffect(() => {
    // Per-container writes → live-reload the focused view (coalesced). NO
    // announcement here: a slow warm pass touching many containers seconds apart
    // defeats the coalesce window and spoke once per source — the chatter the
    // user hit with 8+ calendars. The spoken cue now brackets the whole pass
    // (see the refresh-status listener below).
    const subData = CalFfi.addListener('onCacheUpdated', ({ payload }) => {
      let scope = '';
      try {
        scope = (JSON.parse(payload) as { scope?: string }).scope ?? '';
      } catch {
        return;
      }
      const category = categoryForScope(scope);
      if (category == null) return;
      pending.current.add(category);
      if (timer.current != null) clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        timer.current = null;
        const categories = Array.from(pending.current);
        pending.current.clear();
        for (const cat of categories) busListeners.forEach((l) => l(cat));
      }, COALESCE_MS);
    });

    // ONE polite cue at the start of an external refresh pass + ONE at the end
    // (the user-chosen model), regardless of how many sources refresh in between.
    const subStatus = CalFfi.addListener('onCacheRefreshStatus', ({ status: json }) => {
      let status: CacheRefreshStatus;
      try {
        status = JSON.parse(json) as CacheRefreshStatus;
      } catch {
        return;
      }
      // Publish progress (fetched X of N) app-wide for the sync indicator + the
      // Sync screen — separate from the start/end announcement below.
      setCacheRefreshProgress(status);
      const next = status.refreshing;
      if (next === refreshing.current) return;
      refreshing.current = next;
      AccessibilityInfo.announceForAccessibility(
        t(next ? 'cacheRefresh.refreshing' : 'cacheRefresh.done'),
      );
      // Route through the shared loading coordinator so a refresh pass that
      // overlaps a view load (the common case: an external delete reloads the
      // view AND kicks this pass) is felt as one cue, not two.
      if (next) hapticLoadBegin();
      else hapticLoadEnd();
    });
    return () => {
      subData.remove();
      subStatus.remove();
      if (timer.current != null) clearTimeout(timer.current);
    };
  }, [t]);
}

/**
 * Screen hook: live-reload when the external cache for `category` refreshes.
 * Pass the screen's (stable, useCallback) load fn — it's called coalesced on a
 * relevant background-refresh push. Pair with the screen's existing
 * focus-reload; together they cover "fresh while open" + "fresh on return".
 */
export function useCacheReload(category: CacheCategory, reload: () => void): void {
  useEffect(
    () =>
      subscribeBus((cat) => {
        if (cat === category) reload();
      }),
    [category, reload],
  );
}
