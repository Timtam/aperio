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

import { NavigationContext } from '@react-navigation/native';
import { useCallback, useContext, useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { AppState } from 'react-native';

import { passEndSentence } from '@aperio/shared';

import { announceAround } from '../a11y/speechHold';
import type { CacheRefreshStatus } from '../api/sync';
import {
  cancelWhenPassSettled,
  endCacheUpdates,
  noteCacheUpdated,
  notePassRefreshing,
  runSettledWork,
  subscribeScreenReload,
  whenPassSettled,
} from './cacheReloadBus';
import type { CacheCategory, ScreenFocus } from './cacheReloadBus';
import { setCacheRefreshProgress } from './cacheRefreshProgress';
import { notePassEndSpoken } from './useRefreshErrors';
import { hapticLoadBegin, hapticLoadEnd, loadHapticsPref } from './haptics';
import CalFfi from '../../modules/cal-ffi';

export { notifyDataReload, subscribeCacheReload, whenPassSettled } from './cacheReloadBus';

/**
 * Mount ONCE near the app root. Subscribes to the native external-cache push,
 * coalesces the burst, announces each refreshed category politely, and notifies
 * the bus so the focused view live-reloads.
 */
export function useCacheUpdates(): void {
  const { t } = useTranslation();
  // Last-seen warm-pass state, so we announce only the START and END of a pass.
  const refreshing = useRef(false);
  // The translator rides a ref so the subscription effect can stay mounted
  // ONCE ([] deps): re-running it on a language switch tore the coalescer
  // down mid-window — an armed flush was dropped (pending categories sat
  // until the next cache event, which in an idle session never comes) and
  // `passRefreshing` was force-reset while `refreshing` kept its value, so
  // the throttle stayed off for the rest of a running pass.
  const tRef = useRef(t);
  tRef.current = t;

  // Prime the device-local haptics pref once (default on).
  useEffect(() => {
    void loadHapticsPref();
  }, []);

  useEffect(() => {
    // (Re)subscribe keeps the module flag honest even if a previous mount
    // was torn down mid-pass (e.g. fast refresh in dev).
    notePassRefreshing(refreshing.current);
    // Per-container writes → live-reload the focused view (coalesced +
    // pass-throttled, see scheduleFlush). NO announcement here: a slow warm
    // pass touching many containers seconds apart spoke once per source — the
    // chatter the user hit with 8+ calendars. The spoken cue brackets the
    // whole pass (see the refresh-status listener below).
    const subData = CalFfi.addListener('onCacheUpdated', ({ payload }) => {
      let scope = '';
      try {
        scope = (JSON.parse(payload) as { scope?: string }).scope ?? '';
      } catch {
        return;
      }
      noteCacheUpdated(scope);
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
      // Pass end: the settled state paints now, then what waited for it.
      notePassRefreshing(next);
      // Behind a held sentence (the start check's), not through it. The end
      // names the accounts left not current (decision 180). Without an
      // outcome (it could not be read) it only says the refresh ended, and
      // the warnings follow after the settle window as before.
      const outcome = status.outcome ?? null;
      announceAround(
        next ? tRef.current('cacheRefresh.refreshing') : passEndSentence(outcome, tRef.current),
      );
      if (!next && outcome != null) notePassEndSpoken(outcome);
      // Route through the shared loading coordinator so a refresh pass that
      // overlaps a view load (the common case: an external delete reloads the
      // view AND kicks this pass) is felt as one cue, not two.
      if (next) hapticLoadBegin();
      else hapticLoadEnd();
    });
    // Leaving the foreground: what waits for the pass end runs now. iOS
    // suspends the JS in the background, and the home screen, where the
    // widget is read, is where the user is going.
    const subApp = AppState.addEventListener('change', (state) => {
      if (state !== 'active') runSettledWork();
    });
    return () => {
      subData.remove();
      subStatus.remove();
      subApp.remove();
      endCacheUpdates();
    };
  }, []);
}

/**
 * Screen hook: live-reload when the external cache for `category` refreshes.
 * Pass the screen's (stable, useCallback) load fn — it's called coalesced on a
 * relevant background-refresh push, while the screen is in view; a screen out
 * of view reloads once when it comes back (see `subscribeScreenReload`).
 * Outside a screen (the app root's stores) every push reloads.
 */
export function useCacheReload(category: CacheCategory, reload: () => void): void {
  // `undefined` outside a screen; the hook form would throw there.
  const navigation = useContext(NavigationContext) as ScreenFocus | undefined;
  useEffect(
    () => subscribeScreenReload(category, reload, navigation),
    [category, reload, navigation],
  );
}

/**
 * `work`, deferred while a warm pass runs (see `whenPassSettled`): the
 * returned function is stable for the component's life and runs the latest
 * `work` once at the end of the pass, or at once when none runs. What it
 * deferred is dropped when the component unmounts.
 */
export function useWhenPassSettled(work: () => void): () => void {
  const latest = useRef(work);
  latest.current = work;
  const run = useMemo(() => () => latest.current(), []);
  useEffect(() => () => cancelWhenPassSettled(run), [run]);
  return useCallback(() => whenPassSettled(run), [run]);
}
