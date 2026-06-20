import AsyncStorage from '@react-native-async-storage/async-storage';

// Per-device "last fired" marker for the day-start checkers (the mobile twin of
// the desktop localStorage marker in useCurrentDayKey). Keyed per checker slot
// so a mid-day relaunch doesn't re-run a silent batch (or re-announce) for a day
// already handled. Device-local (NOT synced) — each device runs its own checks.
// Best-effort: a storage failure reads as "never fired" so the gate still runs.

const PREFIX = 'aperio.dayStartFired.';

export async function readFiredDayKey(slot: string): Promise<string | null> {
  try {
    return await AsyncStorage.getItem(`${PREFIX}${slot}`);
  } catch {
    return null;
  }
}

export async function writeFiredDayKey(slot: string, dayKey: string): Promise<void> {
  try {
    await AsyncStorage.setItem(`${PREFIX}${slot}`, dayKey);
  } catch {
    // Storage unavailable; the gate re-runs next eligible launch/foreground.
  }
}
