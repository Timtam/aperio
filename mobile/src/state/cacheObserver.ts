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

// Reminder-scheduler hook — REGISTERED by the scheduler at its module init
// (a direct import here would close a module cycle through the api layer).
// Invoked whenever data lands that can change what should be scheduled: a
// peer sync round applied changes, or an external-cache refresh finished.
let remindersRefreshHook: (() => void) | null = null;
export function setRemindersRefreshHook(cb: () => void): void {
  remindersRefreshHook = cb;
}

/**
 * Non-hook subscription for a MODULE-LEVEL store (a screen uses
 * `useCacheReload`). A store that hydrates once and holds its value in memory
 * — the signature list — has to hear about a sync round that rewrote its row,
 * or every consumer (editors included) keeps the pre-round value until the
 * next launch. Returns the unsubscribe.
 */
export function subscribeCacheReload(category: CacheCategory, cb: () => void): () => void {
  return subscribeBus((cat) => {
    if (cat === category) cb();
  });
}

function subscribeBus(cb: BusListener): () => void {
  busListeners.add(cb);
  return () => {
    busListeners.delete(cb);
  };
}

/** Coalesce window: a burst of per-container events (no warm pass running)
 *  waits this long after the FIRST event before reloading once per category. */
const COALESCE_MS = 700;

/** Max flush cadence while a backend warm pass is in flight. A pass spreads
 *  its per-container emissions over seconds, which used to defeat the short
 *  coalesce window and turn one pass into many full reload waves — each
 *  re-exposing whatever intermediate cache state existed at that moment (the
 *  app-start entry-count oscillation). The pass-end status flushes
 *  immediately, so the settled state paints exactly once; a hung pass can't
 *  starve the UI because the throttle keeps its own cadence. */
const PASS_THROTTLE_MS = 2500;

// Module-level coalescer state, shared by the native listeners (below) and
// notifyDataReload so EVERY reload producer rides the same gating.
const pendingCategories = new Set<CacheCategory>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;
let passRefreshing = false;

function flushPending(): void {
  flushTimer = null;
  if (pendingCategories.size === 0) return;
  const categories = Array.from(pendingCategories);
  pendingCategories.clear();
  for (const cat of categories) busListeners.forEach((l) => l(cat));
  // Fresh data can change reminder-relevant state (provider-side edits,
  // peer-synced tasks) — re-plan the scheduled OS notifications. Coalesced
  // here + debounced in the scheduler, so a warm pass costs one reschedule.
  remindersRefreshHook?.();
}

// Collect-then-flush: the FIRST category of a window arms the timer; later
// ones just accumulate (no per-event reset, so a drip of events can't
// postpone the flush forever and latency stays bounded).
function scheduleFlush(): void {
  if (flushTimer != null) return;
  flushTimer = setTimeout(
    flushPending,
    passRefreshing ? PASS_THROTTLE_MS : COALESCE_MS,
  );
}

/**
 * Ask every screen subscribed via `useCacheReload` to reload, across ALL
 * categories. Use after a CROSS-DEVICE sync round (or onboarding) applies a
 * peer's data: that path writes straight to the local store and never goes
 * through the Host's external `onCacheUpdated` push, so without this the open
 * screens stay stale until the app restarts. Silent — it's a data reload, not
 * the external-refresh cue. Routed through the SAME coalescer as the native
 * push, so a sync round landing mid-warm-pass can't bypass the reload gating.
 */
export function notifyDataReload(): void {
  pendingCategories.add('calendar');
  pendingCategories.add('tasks');
  pendingCategories.add('contacts');
  // Immediate flush: the sync UI just reported applied changes (or the user
  // pulled to refresh) — waiting out a pass throttle here reads as "sync
  // done but the screen is stale". Going through the shared flush still
  // clears `pendingCategories`, so a running pass's own emissions aren't
  // replayed on top.
  if (flushTimer != null) clearTimeout(flushTimer);
  flushPending();
}

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
    passRefreshing = refreshing.current;
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
      const category = categoryForScope(scope);
      if (category == null) return;
      pendingCategories.add(category);
      scheduleFlush();
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
      passRefreshing = next;
      if (!next) {
        // Pass end: paint the settled state NOW instead of waiting out a long
        // throttle window armed mid-pass.
        if (flushTimer != null) clearTimeout(flushTimer);
        flushPending();
      }
      AccessibilityInfo.announceForAccessibility(
        tRef.current(next ? 'cacheRefresh.refreshing' : 'cacheRefresh.done'),
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
      // True unmount (app teardown): flush whatever is pending so no
      // subscriber is left stale, then disarm.
      if (flushTimer != null) clearTimeout(flushTimer);
      flushPending();
      passRefreshing = false;
    };
  }, []);
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
