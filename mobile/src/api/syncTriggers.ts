// JS-driven sync scheduler — the mobile stand-in for the desktop `SyncScheduler`.
// The desktop runs a tokio periodic loop (default 5 min) while the process is
// alive; mobile drives sync from the JS runtime, which is alive only while the
// app is FOREGROUNDED. So sync fires on four signals:
//
//   1. Launch + foreground-resume → a full `syncNow()` (pull peers' changes).
//   2. FOREGROUND periodic         → a full `syncNow()` every `sync.intervalMinutes`
//                                    (default 5, a SYNCED pref) while the app is
//                                    active — the open-app equivalent of the
//                                    desktop periodic loop. Paused off-foreground.
//   3. App background              → a `pushNow()` (flush our pending log before
//                                    the OS suspends us).
//   4. Any local mutation          → a debounced `pushNow()` (the Rust Host already
//                                    appended the SyncEvent; this ships it), so a
//                                    burst of edits coalesces into one push.
//
// What this does NOT do (deliberately, a separate device-gated item): wake the
// app to sync while it's backgrounded/closed. That needs the OS schedulers
// (iOS BGTaskScheduler — unreliable timing; Android WorkManager), which are
// genuinely platform-specific. On launch/foreground we always do a full round,
// so a backgrounded device catches up the moment it's reopened.
//
// Every trigger is best-effort and SILENT: a missing sync target ("not
// configured") or a transient network error must never surface as a disruptive
// error from a background action the user didn't explicitly invoke. Manual
// "Sync now" (the Sync screen) keeps surfacing errors — that path is unchanged.

import { useEffect } from 'react';
import { AppState, type AppStateStatus } from 'react-native';

import { refreshRemindersSoon } from '../reminders/scheduler';
import { getUserPref } from './prefs';
import { pushNow, syncNow, warmCacheOnForeground } from './sync';

/** Debounce window for the post-mutation push (matches the desktop scheduler's
 *  debounce: coalesce a rapid burst of edits into a single push). */
const MUTATION_PUSH_DEBOUNCE_MS = 2000;

/** The synced sync-interval pref + its fallback, mirroring sync_engine's
 *  `PREF_SYNC_INTERVAL_MINUTES` / `DEFAULT_SYNC_INTERVAL_MINUTES`. A floor keeps
 *  a stray tiny value from hammering the network. */
const PREF_SYNC_INTERVAL_MINUTES = 'sync.intervalMinutes';
const DEFAULT_SYNC_INTERVAL_MINUTES = 5;
const MIN_SYNC_INTERVAL_MINUTES = 1;

/** Resolve the foreground periodic interval (ms) from the synced pref, clamped
 *  to a sane floor; falls back to the default on absence / a bad value. */
async function readSyncIntervalMs(): Promise<number> {
  let minutes = DEFAULT_SYNC_INTERVAL_MINUTES;
  try {
    const raw = await getUserPref(PREF_SYNC_INTERVAL_MINUTES);
    const parsed = raw != null ? Number.parseInt(raw, 10) : NaN;
    if (Number.isFinite(parsed)) {
      minutes = Math.max(MIN_SYNC_INTERVAL_MINUTES, parsed);
    }
  } catch {
    // Pref read failed — fall back to the default.
  }
  return minutes * 60_000;
}

let pushTimer: ReturnType<typeof setTimeout> | null = null;

/** Schedule a debounced background push. Call after any local mutation; rapid
 *  successive calls collapse into one push `MUTATION_PUSH_DEBOUNCE_MS` after the
 *  last. Errors are swallowed (unconfigured target / offline are expected). */
export function scheduleBackgroundPush(): void {
  if (pushTimer != null) clearTimeout(pushTimer);
  pushTimer = setTimeout(() => {
    pushTimer = null;
    // The debounced post-mutation push — tagged 'kick' in the sync log.
    void pushNow('kick').catch(() => undefined);
  }, MUTATION_PUSH_DEBOUNCE_MS);
  // A local mutation may have added/changed/removed a reminder — roll the
  // scheduled OS notifications forward too (debounced, best-effort).
  refreshRemindersSoon();
}

/** Flush any pending debounced push immediately (used when backgrounding, so we
 *  don't lose the window). Safe to call when nothing is pending. */
function flushPendingPush(): void {
  if (pushTimer != null) {
    clearTimeout(pushTimer);
    pushTimer = null;
  }
  // The background flush before the OS suspends us — tagged 'app_exit'.
  void pushNow('app_exit').catch(() => undefined);
}

/**
 * Wire the sync scheduler. Mount once near the app root. Runs a full round on
 * launch + every foreground-resume, a foreground periodic round every
 * `sync.intervalMinutes` while active, and a push flush on background. All
 * best-effort + silent.
 */
export function useSyncTriggers(): void {
  useEffect(() => {
    // The foreground periodic timer — the open-app equivalent of the desktop's
    // periodic sync loop. Only ticks while the app is active (cleared on
    // background); re-armed on each foreground at the current synced interval.
    let periodic: ReturnType<typeof setInterval> | null = null;
    const stopPeriodic = () => {
      if (periodic != null) {
        clearInterval(periodic);
        periodic = null;
      }
    };
    const startPeriodic = () => {
      stopPeriodic();
      void readSyncIntervalMs().then((ms) => {
        // The app may have backgrounded while the (async) pref read was in
        // flight — don't arm a timer that should be paused.
        if (AppState.currentState === 'active') {
          periodic = setInterval(() => {
            void syncNow('periodic').catch(() => undefined);
          }, ms);
        }
      });
    };

    // Initial pull on launch (the Host opens lazily on the first bridge call)
    // + a warm pass over the external SWR caches (the mobile stand-in for the
    // desktop periodic warm loop) + arm the foreground periodic timer (the app
    // starts active).
    void syncNow('app_start').catch(() => undefined);
    void warmCacheOnForeground().catch(() => undefined);
    startPeriodic();

    const onChange = (state: AppStateStatus) => {
      if (state === 'active') {
        // Foreground-resume full round + external-cache warm + re-arm the
        // periodic timer (the synced interval may have changed on another
        // device while we were away).
        void syncNow('periodic').catch(() => undefined);
        void warmCacheOnForeground().catch(() => undefined);
        startPeriodic();
      } else {
        // Off-foreground: pause the periodic timer (no JS runs reliably anyway,
        // and it saves battery); flush a final push when going to background.
        stopPeriodic();
        if (state === 'background') flushPendingPush();
      }
    };
    const sub = AppState.addEventListener('change', onChange);
    return () => {
      sub.remove();
      stopPeriodic();
    };
  }, []);
}
