import { useCallback, useEffect, useState } from 'react';
import { Platform } from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';
import * as BackgroundTask from 'expo-background-task';
import * as TaskManager from 'expo-task-manager';

import CalFfi from '../../modules/cal-ffi';

import { getUserPref } from '../api/prefs';
import { cacheRefreshStatus, refreshExternalCache, syncNow, syncStatus } from '../api/sync';
import { rescheduleReminders } from '../reminders/scheduler';
import { refreshWidgetSnapshot } from './widgetSnapshot';

// OS-scheduled background sync — the piece syncTriggers.ts explicitly deferred:
// wake the app while it's backgrounded/closed to run a sync round, so a peer's
// changes and any new reminders land without the user reopening the app. iOS
// uses BGTaskScheduler, Android WorkManager (both via expo-background-task). The
// timing is the OS's call — Android enforces a >= 15-minute floor, iOS picks its
// own windows (often overnight, battery/network permitting) — so this is a
// best-effort catch-up, the OS-driven cousin of the foreground periodic loop in
// syncTriggers.ts, NOT a precise scheduler.
//
// DEVICE-LOCAL on/off (default on), stored in AsyncStorage like the haptics /
// app-badge prefs — a per-device behaviour, not synced. The setter (un)registers
// the OS task immediately; `initBackgroundSync` reconciles registration to the
// stored pref on launch.

export const BACKGROUND_SYNC_TASK = 'aperio.background-sync';

const KEY = 'aperio.backgroundSync.enabled';
const DEFAULT_ENABLED = true;
// The synced foreground interval doubles as the requested background cadence,
// clamped to the WorkManager floor (iOS ignores fine-grained values anyway).
const PREF_SYNC_INTERVAL_MINUTES = 'sync.intervalMinutes';
const MIN_BACKGROUND_INTERVAL_MINUTES = 15;

let cached = DEFAULT_ENABLED;

// Define the task at module scope: Expo requires the handler to be registered as
// the JS bundle loads, including in the headless background context. The handler
// runs ONE guarded sync round + reschedules local reminders for any freshly
// pulled items. Silent + best-effort, exactly like the foreground auto-rounds —
// a missing target or a transient error must never surface.
TaskManager.defineTask(BACKGROUND_SYNC_TASK, async () => {
  try {
    // Ask the calendar and task PROVIDERS for anything new, first, because it is
    // the slowest part and everything below reads what it lands.
    //
    // This used to be missing entirely, and its absence is easy to miss: the
    // round ran `syncNow`, which is the DEVICE-TO-DEVICE engine — it carries a
    // peer's edits over WebDAV and knows nothing about iCloud or Google. So a
    // background round refreshed nothing from the accounts and then wrote a
    // widget snapshot out of an untouched cache. Correct-looking, and always one
    // warm pass behind.
    await refreshExternalCache().catch(() => undefined);
    if ((await syncStatus()).configured) {
      await syncNow('background');
    }
    await waitForExternalRefresh();
    // A pull may have added/changed events → reschedule the local reminders so
    // their notifications still fire on time. Best-effort: a reminder hiccup
    // must not fail the (successful) sync round.
    await rescheduleReminders().catch(() => undefined);
    // Same reasoning for the home-screen widgets, and this is the ONLY path that
    // reaches them without the user opening the app — the in-app triggers all
    // need a foreground. Without it a phone left alone overnight would show
    // yesterday's agenda until it was next unlocked and the app opened.
    await refreshWidgetSnapshot().catch(() => undefined);
    return BackgroundTask.BackgroundTaskResult.Success;
  } catch {
    return BackgroundTask.BackgroundTaskResult.Failed;
  }
});

/** Longest we will sit waiting for the provider pass before writing the
 *  snapshot anyway.
 *
 *  A budget rather than a completion check, because the shorter of the two OS
 *  task classes gives about thirty seconds in total and being killed mid-round
 *  is worse than being a little stale: iOS counts an expired task as a failure
 *  and hands out less time next time. Whatever has landed by then is written;
 *  the containers still in flight are picked up by the following round, since
 *  the warm pass persists each one as it completes rather than all at the end. */
const EXTERNAL_REFRESH_BUDGET_MS = 15_000;
const EXTERNAL_REFRESH_POLL_MS = 500;
/** How long "not refreshing" still means "has not started yet".
 *
 *  The kick returns before the pass does anything, so an immediate status read
 *  says `refreshing: false` — which is indistinguishable from "finished" and
 *  would make the wait a no-op. After this, "not refreshing" is taken at face
 *  value, which is also the honest answer on a device with no external accounts
 *  at all. */
const EXTERNAL_REFRESH_START_GRACE_MS = 2_000;

/** Wait for the warm pass to finish, or for the budget to run out. Never
 *  throws; a status read that fails simply ends the wait. */
async function waitForExternalRefresh(): Promise<void> {
  const deadline = Date.now() + EXTERNAL_REFRESH_BUDGET_MS;
  const startedBy = Date.now() + EXTERNAL_REFRESH_START_GRACE_MS;
  let seenRunning = false;
  while (Date.now() < deadline) {
    let refreshing: boolean;
    try {
      refreshing = (await cacheRefreshStatus()).refreshing;
    } catch {
      return;
    }
    if (refreshing) {
      seenRunning = true;
    } else if (seenRunning || Date.now() > startedBy) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, EXTERNAL_REFRESH_POLL_MS));
  }
}

async function backgroundIntervalMinutes(): Promise<number> {
  let minutes = MIN_BACKGROUND_INTERVAL_MINUTES;
  try {
    const raw = await getUserPref(PREF_SYNC_INTERVAL_MINUTES);
    const parsed = raw != null ? Number.parseInt(raw, 10) : NaN;
    if (Number.isFinite(parsed)) {
      minutes = Math.max(MIN_BACKGROUND_INTERVAL_MINUTES, parsed);
    }
  } catch {
    // Pref read failed — fall back to the floor.
  }
  return minutes;
}

/** Register the OS background-sync task (idempotent). No-op when the OS reports
 *  the capability unavailable (e.g. iOS Background App Refresh turned off). */
export async function registerBackgroundSync(): Promise<void> {
  try {
    const status = await BackgroundTask.getStatusAsync();
    if (status !== BackgroundTask.BackgroundTaskStatus.Available) return;
    const minutes = await backgroundIntervalMinutes();
    await BackgroundTask.registerTaskAsync(BACKGROUND_SYNC_TASK, {
      minimumInterval: minutes,
    });
    // iOS only, and the reason it exists is worth stating where it is called:
    // expo-background-task asks for a PROCESSING task, the class iOS runs when
    // the device is idle and preferably charging — overnight, in practice. This
    // arms a second wake-up of the SHORT class, which the system spreads across
    // the day. Both run the same handler above.
    if (Platform.OS === 'ios') {
      await CalFfi.enableBackgroundRefresh(minutes).catch(() => undefined);
    }
  } catch {
    // Best-effort; registration can fail on an unsupported platform.
  }
}

/** Unregister the OS background-sync task (idempotent). */
export async function unregisterBackgroundSync(): Promise<void> {
  try {
    if (await TaskManager.isTaskRegisteredAsync(BACKGROUND_SYNC_TASK)) {
      await BackgroundTask.unregisterTaskAsync(BACKGROUND_SYNC_TASK);
    }
  } catch {
    // Best-effort.
  }
  // Outside the guard above: the short wake-up is armed separately, so it has to
  // be cancelled even when unregistering the OS task threw or found nothing.
  if (Platform.OS === 'ios') {
    await CalFfi.disableBackgroundRefresh().catch(() => undefined);
  }
}

/** Load the stored pref and reconcile the OS registration to match. Call once on
 *  app start (from the sync-triggers hook). */
export async function initBackgroundSync(): Promise<void> {
  try {
    const v = await AsyncStorage.getItem(KEY);
    if (v != null) cached = v === 'true';
  } catch {
    // Keep the default.
  }
  if (cached) await registerBackgroundSync();
  else await unregisterBackgroundSync();
}

async function persist(enabled: boolean): Promise<void> {
  cached = enabled;
  try {
    await AsyncStorage.setItem(KEY, String(enabled));
  } catch {
    // Best-effort.
  }
  if (enabled) await registerBackgroundSync();
  else await unregisterBackgroundSync();
}

/** Settings hook: the current value + a setter that persists and (un)registers
 *  the OS task immediately. Mirrors `useHapticsPref` / `useAppBadgePref`. */
export function useBackgroundSyncPref(): [boolean, (next: boolean) => void] {
  const [enabled, setEnabled] = useState(cached);
  useEffect(() => {
    void AsyncStorage.getItem(KEY).then((v) => {
      if (v != null) setEnabled(v === 'true');
    });
  }, []);
  const set = useCallback((next: boolean) => {
    setEnabled(next);
    void persist(next);
  }, []);
  return [enabled, set];
}
