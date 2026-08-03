import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';
import * as BackgroundTask from 'expo-background-task';
import * as TaskManager from 'expo-task-manager';

import { getUserPref } from '../api/prefs';
import { syncNow, syncStatus } from '../api/sync';
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
    if (!(await syncStatus()).configured) {
      // No sync target configured — nothing to do, but not a failure.
      return BackgroundTask.BackgroundTaskResult.Success;
    }
    await syncNow('background');
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
    await BackgroundTask.registerTaskAsync(BACKGROUND_SYNC_TASK, {
      minimumInterval: await backgroundIntervalMinutes(),
    });
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
