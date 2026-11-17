import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';

// Device-local on/off switch for the Live Activities of appointments
// (DESIGN-live-activities.md, decision 7: default ON). Unlike the badge, there
// is no synchronous cache: the reminder pass reads it fresh every time — also
// in the background task, where no settings screen has ever loaded it, and
// where a stale "on" would start or keep activities the user switched off.

const KEY = 'aperio.liveActivities.enabled';

/** Whether Live Activities are on, read now. Default on. */
export async function readLiveActivitiesEnabled(): Promise<boolean> {
  try {
    const v = await AsyncStorage.getItem(KEY);
    return v == null ? true : v === 'true';
  } catch {
    return true;
  }
}

/** Settings hook: the current value and a setter that stores it. The caller
 *  re-plans after a flip, which ends or starts the activities. */
export function useLiveActivitiesPref(): [boolean, (next: boolean) => Promise<void>] {
  const [enabled, setEnabled] = useState(true);
  useEffect(() => {
    void readLiveActivitiesEnabled().then(setEnabled);
  }, []);
  const set = useCallback(async (next: boolean) => {
    setEnabled(next);
    try {
      await AsyncStorage.setItem(KEY, String(next));
    } catch {
      // Best-effort; the next read keeps the old value.
    }
  }, []);
  return [enabled, set];
}

// The trial "end ahead" (Toni, 2026-11-17; DESIGN-live-activities.md,
// "Versuch: vorab beenden"): once nothing follows in its chain and its end is
// within four hours, the next full pass that sees the activity running ends it
// with iOS removing it at the end. Off by default; device-local, read fresh by
// every pass like the switch above.
const END_EARLY_KEY = 'aperio.liveActivities.endEarly';

/** Whether activities are ended ahead (the trial), read now. Default off. */
export async function readLiveActivityEndEarly(): Promise<boolean> {
  try {
    return (await AsyncStorage.getItem(END_EARLY_KEY)) === 'true';
  } catch {
    return false;
  }
}

/** Settings hook for the trial: the current value and a setter that stores
 *  it. The caller re-plans after a flip. */
export function useLiveActivityEndEarlyPref(): [boolean, (next: boolean) => Promise<void>] {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    void readLiveActivityEndEarly().then(setEnabled);
  }, []);
  const set = useCallback(async (next: boolean) => {
    setEnabled(next);
    try {
      await AsyncStorage.setItem(END_EARLY_KEY, String(next));
    } catch {
      // Best-effort; the next read keeps the old value.
    }
  }, []);
  return [enabled, set];
}
