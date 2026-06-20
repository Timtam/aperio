import { getUserPref, setUserPref } from '../api/prefs';

// The week-start preference, SYNCED across the user's devices via the
// `view.weekStart` user-pref (a §19.2.1 always-sync key) — the mobile twin of
// the desktop ViewState's `weekStartsOn`. Drives the Week view's visual day
// order (Monday by default); calendar week NUMBERS stay ISO-8601 (Monday-based)
// regardless. Shared by WeekScreen (the consumer) and SettingsScreen (the
// picker), the same read/write/parse pattern as ./language.

/** Which weekday a week visually starts on (0 = Sunday … 6 = Saturday). */
export type WeekStart = 0 | 1 | 2 | 3 | 4 | 5 | 6;

const WEEK_START_PREF = 'view.weekStart';

/** Normalise the stored pref string (`"0".."6"`) → a {@link WeekStart},
 *  defaulting to Monday (ISO) when unset or out of range. */
export function parseWeekStart(stored: string | null): WeekStart {
  const n = stored == null ? NaN : Number(stored);
  return Number.isInteger(n) && n >= 0 && n <= 6 ? (n as WeekStart) : 1;
}

/** Read the synced week-start, or Monday when unset/unreadable. */
export async function readWeekStart(): Promise<WeekStart> {
  try {
    return parseWeekStart(await getUserPref(WEEK_START_PREF));
  } catch {
    return 1;
  }
}

/** Persist the week-start to the synced pref (propagates on the next round).
 *  Best-effort — the caller's local state still reflects the choice this run. */
export async function writeWeekStart(value: WeekStart): Promise<void> {
  try {
    await setUserPref(WEEK_START_PREF, String(value));
  } catch {
    // Ignore — the chosen start still applies for this session.
  }
}
