import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';

// Device-local on/off toggle for the app-icon badge (default ON). Like the
// haptics pref it's stored per device in AsyncStorage (the badge is a per-device
// display preference, not synced) and mirrored into a module-level cache so the
// root badge hook can read it synchronously. A listener set lets that hook
// recompute the badge the instant the toggle flips in Settings.

const KEY = 'aperio.appBadge.enabled';
let cached = true;
const listeners = new Set<() => void>();

/** Load the stored pref into the cache. The root hook calls this on mount. */
export async function loadAppBadgePref(): Promise<void> {
  try {
    const v = await AsyncStorage.getItem(KEY);
    if (v != null) cached = v === 'true';
  } catch {
    // Best-effort; the default (on) stays.
  }
}

/** The current toggle value, read synchronously from the cache. */
export function appBadgeEnabled(): boolean {
  return cached;
}

/** Subscribe to toggle flips so the root badge hook can recompute immediately. */
export function subscribeAppBadge(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

async function persist(enabled: boolean): Promise<void> {
  cached = enabled;
  listeners.forEach((l) => l());
  try {
    await AsyncStorage.setItem(KEY, String(enabled));
  } catch {
    // Best-effort.
  }
}

/** Settings hook: the current value + a setter that persists + notifies the
 *  badge hook so the icon updates immediately. */
export function useAppBadgePref(): [boolean, (next: boolean) => void] {
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
