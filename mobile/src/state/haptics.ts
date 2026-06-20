import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';
import * as Haptics from 'expo-haptics';

// Haptic feedback for the external-refresh start/end cues, with a DEVICE-LOCAL
// on/off toggle (default ON). The setting is stored per device in AsyncStorage
// (not synced — it's a per-device hardware preference) and mirrored into a
// module-level cache so the (frequent) sync cues fire without an async read.
// expo-haptics exposes presets, not custom patterns, so "different vibrations"
// = a light IMPACT to start vs. a success NOTIFICATION to finish.

const KEY = 'aperio.haptics.enabled';
let cached = true;

/** Load the stored pref into the cache. Call once on app start. */
export async function loadHapticsPref(): Promise<void> {
  try {
    const v = await AsyncStorage.getItem(KEY);
    if (v != null) cached = v === 'true';
  } catch {
    // Best-effort; the default (on) stays.
  }
}

async function persist(enabled: boolean): Promise<void> {
  cached = enabled;
  try {
    await AsyncStorage.setItem(KEY, String(enabled));
  } catch {
    // Best-effort.
  }
}

/** Settings hook: the current value + a setter that persists + updates the
 *  cache so the cues honour it immediately. */
export function useHapticsPref(): [boolean, (next: boolean) => void] {
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

/** A light tap as an external refresh pass begins. No-op when disabled. */
export function hapticSyncStart(): void {
  if (!cached) return;
  void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(() => undefined);
}

/** A distinct success buzz as the pass finishes. No-op when disabled. */
export function hapticSyncEnd(): void {
  if (!cached) return;
  void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(
    () => undefined,
  );
}
