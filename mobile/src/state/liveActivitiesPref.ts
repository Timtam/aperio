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
