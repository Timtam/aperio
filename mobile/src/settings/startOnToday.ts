import { getUserPref, setUserPref } from '../api/prefs';

// "Start on today" — SYNCED across the user's devices via the
// `view.startOnToday` user-pref (a §19.2.1 always-sync key), the mobile twin of
// the desktop ViewState's `startOnToday`. When on, every calendar view opens on
// TODAY at app launch instead of restoring the last-opened day (the default).
// App.tsx reads it during the nav-state restore and rewrites the restored
// routes' `anchor` params to today; GeneralSettingsScreen is the toggle.

const START_ON_TODAY_PREF = 'view.startOnToday';

/** Read the synced start-on-today pref; false (restore last day) when unset. */
export async function readStartOnToday(): Promise<boolean> {
  try {
    return (await getUserPref(START_ON_TODAY_PREF)) === 'true';
  } catch {
    return false;
  }
}

/** Persist the pref to the synced key. Best-effort — a write failure leaves the
 *  caller's local toggle state reflecting the choice for this session. */
export async function writeStartOnToday(value: boolean): Promise<void> {
  try {
    await setUserPref(START_ON_TODAY_PREF, value ? 'true' : 'false');
  } catch {
    // Ignore — the choice still applies until the next launch.
  }
}
