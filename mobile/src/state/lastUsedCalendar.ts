import AsyncStorage from '@react-native-async-storage/async-storage';

// The calendar the user most recently created an event on — the mobile twin of
// the desktop `src/components/lastUsedCalendar.ts` (there: localStorage). Read
// when a create surface picks its default calendar, so anyone with more than
// two calendars wired up doesn't land on the first one every time. Device-local
// (NOT synced), same scope as the desktop key. Best-effort: an unreadable store
// just falls back to the first writable calendar, as before.

const LAST_USED_CALENDAR_KEY = 'aperio.lastUsedCalendar.v1';

export async function readLastUsedCalendar(): Promise<string | null> {
  try {
    return await AsyncStorage.getItem(LAST_USED_CALENDAR_KEY);
  } catch {
    return null;
  }
}

export async function writeLastUsedCalendar(id: string): Promise<void> {
  try {
    await AsyncStorage.setItem(LAST_USED_CALENDAR_KEY, id);
  } catch {
    // Best effort — the next create simply defaults as it did before.
  }
}
