import AsyncStorage from '@react-native-async-storage/async-storage';

// Remembers the user's last-picked task list so a multi-list setup doesn't reset
// the new-task / quick-add picker to taskLists[0] on every open — the mobile
// twin of the desktop lastUsedTaskList.ts (AsyncStorage instead of localStorage,
// hence async). Device-local: which list you last typed into is a per-device UI
// convenience, not synced state. Best-effort on both ends.

const KEY = 'aperio.lastUsedTaskList.v1';

export async function readLastUsedTaskList(): Promise<string | null> {
  try {
    return await AsyncStorage.getItem(KEY);
  } catch {
    return null;
  }
}

export async function writeLastUsedTaskList(id: string): Promise<void> {
  try {
    await AsyncStorage.setItem(KEY, id);
  } catch {
    // Best effort — the next open just falls back to the first writable list.
  }
}
