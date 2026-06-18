// JS-driven background sync triggers — the mobile stand-in for the desktop
// `SyncScheduler`. We deliberately do NOT port the desktop's periodic timer
// (iOS has no reliable periodic background execution); instead sync is driven
// by three signals, matching the full-parity plan:
//
//   1. App foreground-resume  → a full `syncNow()` (pull peers' changes).
//   2. App background          → a `pushNow()` (flush our pending log before
//                                the OS suspends us).
//   3. Any local mutation      → a debounced `pushNow()` (the Rust Host already
//                                appended the SyncEvent; this ships it), so a
//                                burst of edits coalesces into one push.
//
// Every trigger is best-effort and SILENT: a missing sync target ("not
// configured") or a transient network error must never surface as a disruptive
// error from a background action the user didn't explicitly invoke. Manual
// "Sync now" (the Sync screen) keeps surfacing errors — that path is unchanged.

import { useEffect } from 'react';
import { AppState, type AppStateStatus } from 'react-native';

import { refreshRemindersSoon } from '../reminders/scheduler';
import { pushNow, syncNow } from './sync';

/** Debounce window for the post-mutation push (matches the desktop scheduler's
 *  debounce: coalesce a rapid burst of edits into a single push). */
const MUTATION_PUSH_DEBOUNCE_MS = 2000;

let pushTimer: ReturnType<typeof setTimeout> | null = null;

/** Schedule a debounced background push. Call after any local mutation; rapid
 *  successive calls collapse into one push `MUTATION_PUSH_DEBOUNCE_MS` after the
 *  last. Errors are swallowed (unconfigured target / offline are expected). */
export function scheduleBackgroundPush(): void {
  if (pushTimer != null) clearTimeout(pushTimer);
  pushTimer = setTimeout(() => {
    pushTimer = null;
    void pushNow().catch(() => undefined);
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
  void pushNow().catch(() => undefined);
}

/**
 * Wire the AppState-driven triggers. Mount once near the app root. On the
 * initial mount and on every foreground-resume it runs a full round; on
 * background it flushes a push. All best-effort + silent.
 */
export function useSyncTriggers(): void {
  useEffect(() => {
    // Initial pull on launch (the Host opens lazily on the first bridge call).
    void syncNow().catch(() => undefined);

    const onChange = (state: AppStateStatus) => {
      if (state === 'active') {
        void syncNow().catch(() => undefined);
      } else if (state === 'background') {
        flushPendingPush();
      }
    };
    const sub = AppState.addEventListener('change', onChange);
    return () => sub.remove();
  }, []);
}
