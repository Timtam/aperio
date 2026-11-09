// JS-driven sync scheduler — the mobile stand-in for the desktop `SyncScheduler`.
// The desktop runs a tokio periodic loop (default 5 min) while the process is
// alive; mobile drives sync from the JS runtime, which is alive only while the
// app is FOREGROUNDED. So sync fires on four signals:
//
//   1. Launch + foreground-resume → a round: refresh the external data and,
//                                    with a sync target, a full `syncNow()`.
//   2. FOREGROUND periodic         → the same round every refresh interval
//                                    (`sync.intervalMinutes`, "Aktualisierungs-
//                                    Intervall" under Allgemein; default 5, a
//                                    SYNCED pref) while the app is active, with
//                                    or without a sync target
//                                    (`periodicRound.ts`). Paused off-foreground.
//   3. App background              → a `pushNow()` (flush our pending log before
//                                    the OS suspends us).
//   4. Any local mutation          → a debounced `pushNow()` (the Rust Host already
//                                    appended the SyncEvent; this ships it), so a
//                                    burst of edits coalesces into one push.
//
// OS-scheduled background sync (waking the app to sync while it's
// backgrounded/closed) now lives in state/backgroundSync.ts and is registered
// here on launch via initBackgroundSync(). It runs on the OS schedulers (iOS
// BGTaskScheduler — system-chosen windows, often overnight; Android WorkManager
// — a >= 15-min floor), so it's a best-effort catch-up, NOT a replacement for
// the launch/foreground full rounds: a reopened device still catches up at once.
//
// Every trigger is best-effort and SILENT: a missing sync target ("not
// configured") or a transient network error must never surface as a disruptive
// error from a background action the user didn't explicitly invoke. Manual
// "Sync now" (the Sync screen) keeps surfacing errors — that path is unchanged.

import { useEffect } from 'react';
import { AppState, type AppStateStatus } from 'react-native';

import { refreshRemindersSoon } from '../reminders/scheduler';
import { followBackgroundInterval, initBackgroundSync } from '../state/backgroundSync';
import { createRoundLoop, runRound } from './periodicRound';
import { pushNow, syncNow, syncStatus, warmCacheOnForeground } from './sync';

/** Debounce window for the post-mutation push (matches the desktop scheduler's
 *  debounce: coalesce a rapid burst of edits into a single push). */
const MUTATION_PUSH_DEBOUNCE_MS = 2000;

/** The one refresh interval in minutes, as the host resolves it
 *  (`sync.intervalMinutes`, clamped and defaulted in Rust). It is there with
 *  or without a sync target. */
async function refreshIntervalMinutes(): Promise<number> {
  return (await syncStatus()).interval_minutes;
}

// Auto (non-user-invoked) rounds run only when a sync target is configured — the
// mobile twin of the desktop scheduler, which SKIPS the round when there is no
// adapter. Without this, every launch/foreground/periodic tick hit the engine
// with "no sync adapter configured", which the Host latched as a failure; after
// a few it tripped `sustained_failure`, surfacing a spurious "sync has failed
// several times" banner + a Settings-tab error badge with no target ever set.
// Manual "Sync now" (the Sync screen) stays unguarded. Best-effort + silent.
async function autoSyncIfConfigured(reason: Parameters<typeof syncNow>[0]): Promise<void> {
  try {
    if (!(await syncStatus()).configured) return;
    await syncNow(reason);
  } catch {
    // missing target / transient network error — silent, as a background action
    // the user didn't invoke must never surface an error.
  }
}

async function autoPushIfConfigured(reason: Parameters<typeof pushNow>[0]): Promise<void> {
  try {
    if (!(await syncStatus()).configured) return;
    await pushNow(reason);
  } catch {
    // silent — see autoSyncIfConfigured.
  }
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
    void autoPushIfConfigured('kick');
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
  void autoPushIfConfigured('app_exit');
}

/** The foreground round loop, one per app (see `periodicRound.ts`). Each round
 *  refreshes the external data — with or without a sync target — and syncs when
 *  a target is set up. */
const roundLoop = createRoundLoop({
  // Fire-and-forget on the Host worker; per-container `cache_updated`
  // re-renders any open view.
  refresh: () => warmCacheOnForeground(),
  syncIfConfigured: () => autoSyncIfConfigured('periodic'),
  intervalMinutes: refreshIntervalMinutes,
  isActive: () => AppState.currentState === 'active',
  setTimer: (run, ms) => setTimeout(run, ms),
  clearTimer: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
});

/** After a launch or foreground round: the timer, and the OS background task
 *  at the cadence the round may have brought from another device. */
function afterForegroundRound(): void {
  roundLoop.start();
  void followBackgroundInterval();
}

/** The refresh interval changed here (Einstellungen → Allgemein): measure the
 *  wait for the next round again from now. */
export function rearmPeriodicRound(): void {
  if (AppState.currentState === 'active') roundLoop.rearm();
}

/**
 * Wire the sync scheduler. Mount once near the app root. Runs a round on
 * launch and on every return to the foreground, a round every refresh interval
 * while active, and a push flush on background. A round refreshes the external
 * data and, with a sync target, syncs. All best-effort + silent.
 */
export function useSyncTriggers(): void {
  useEffect(() => {
    // The launch round (the Host opens lazily on the first bridge call), then
    // the timer (the app starts active). Armed once the round has settled, so
    // its wait is read after the round's pull: an interval set on another
    // device meanwhile paces the very next wait.
    void runRound({
      refresh: () => warmCacheOnForeground(),
      syncIfConfigured: () => autoSyncIfConfigured('app_start'),
    }).finally(afterForegroundRound);
    // Reconcile the OS background-sync registration to the device-local pref —
    // registers (or removes) the BGTaskScheduler/WorkManager task that wakes us
    // to sync while backgrounded/closed.
    void initBackgroundSync();

    const onChange = (state: AppStateStatus) => {
      if (state === 'active') {
        // Back in the foreground: a round, and the timer again once it has
        // settled, at the interval it brought (it may have changed on another
        // device meanwhile).
        void runRound({
          refresh: () => warmCacheOnForeground(),
          syncIfConfigured: () => autoSyncIfConfigured('periodic'),
        }).finally(afterForegroundRound);
      } else {
        // Off-foreground: pause the timer (no JS runs reliably anyway, and it
        // saves battery); flush a final push when going to background.
        roundLoop.stop();
        if (state === 'background') flushPendingPush();
      }
    };
    const sub = AppState.addEventListener('change', onChange);
    return () => {
      sub.remove();
      roundLoop.stop();
    };
  }, []);
}
