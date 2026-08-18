import { useEffect, useState } from 'react';

import { getUserPref, setUserPref } from '../api/prefs';

/**
 * How far one offer in the quick-time popup moves — the mobile twin of the
 * desktop `editor.timeStepMinutes` preference, same key, so the two agree.
 *
 * On the desktop this drives the `step` of an `<input type="time">`. It cannot
 * do that here: Expo's DateTimePicker documents `minuteInterval` as NOT
 * SUPPORTED, so the native wheel always moves a minute at a time. The setting
 * instead decides which times the quick-time popup offers — see
 * `components/QuickTimeButton`.
 *
 * One module-level store with a listener fan-out, like the other mobile pref
 * hooks: a change on the settings screen reaches an open editor at once, and
 * hydration never writes back.
 */

const PREF_KEY = 'editor.timeStepMinutes';
export const TIME_STEP_CHOICES = [1, 5, 10, 15, 30] as const;
export type TimeStepMinutes = (typeof TIME_STEP_CHOICES)[number];
export const DEFAULT_TIME_STEP: TimeStepMinutes = 15;

function isValid(n: number): n is TimeStepMinutes {
  return (TIME_STEP_CHOICES as readonly number[]).includes(n);
}

let cache: TimeStepMinutes = DEFAULT_TIME_STEP;
let loaded = false;
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

async function hydrate(): Promise<void> {
  if (loaded) return;
  if (loading) return loading;
  loading = (async () => {
    try {
      const raw = await getUserPref(PREF_KEY);
      const n = Number(raw);
      if (raw != null && isValid(n)) cache = n;
    } catch {
      // Host unreachable during init — keep the default.
    } finally {
      loaded = true;
      loading = null;
      listeners.forEach((l) => l());
    }
  })();
  return loading;
}

/** Persist a user-initiated change and tell every listener. */
export function setTimeStep(next: TimeStepMinutes): void {
  cache = next;
  loaded = true;
  listeners.forEach((l) => l());
  void setUserPref(PREF_KEY, String(next)).catch(() => {
    // The in-memory value already reflects intent; the next launch re-reads.
  });
}

export function useTimeStep(): TimeStepMinutes {
  const [value, setValue] = useState<TimeStepMinutes>(cache);
  useEffect(() => {
    const listener = () => setValue(cache);
    listeners.add(listener);
    void hydrate().then(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);
  return value;
}
