import { getUserPref, setUserPref } from '../api/prefs';

// The week-start preference, SYNCED across the user's devices via the
// `view.weekStart` user-pref (a §19.2.1 always-sync key) — the mobile twin of
// the desktop ViewState's `weekStartsOn`. Drives the Week view's day order
// (Monday by default) and, with it, the week's number: the ISO week most of
// the displayed days fall in (`cal_core::calendar_week`). Read by WeekScreen,
// MonthScreen and EventsScreen, set by SettingsScreen (the picker), the same
// read/write/parse pattern as ./language.
//
// The last value read or written is kept here, and the app reads it once at
// launch (see ./weekNumbers): a screen then starts on the stored start day
// instead of Monday, and does not announce a changed heading right after
// opening.

/** Which weekday a week visually starts on (0 = Sunday … 6 = Saturday). */
export type WeekStart = 0 | 1 | 2 | 3 | 4 | 5 | 6;

const WEEK_START_PREF = 'view.weekStart';

let known: WeekStart | null = null;

/** Normalise the stored pref string (`"0".."6"`) → a {@link WeekStart},
 *  defaulting to Monday (ISO) when unset or out of range. */
export function parseWeekStart(stored: string | null): WeekStart {
  const n = stored == null ? NaN : Number(stored);
  return Number.isInteger(n) && n >= 0 && n <= 6 ? (n as WeekStart) : 1;
}

/** The last known week start, without waiting: the stored one once read,
 *  else Monday. */
export function peekWeekStart(): WeekStart {
  return known ?? 1;
}

/** Read the synced week-start, or Monday when unset/unreadable. */
export async function readWeekStart(): Promise<WeekStart> {
  try {
    known = parseWeekStart(await getUserPref(WEEK_START_PREF));
  } catch {
    return peekWeekStart();
  }
  return known;
}

/** Persist the week-start to the synced pref (propagates on the next round).
 *  Best-effort — the caller's local state still reflects the choice this run. */
export async function writeWeekStart(value: WeekStart): Promise<void> {
  known = value;
  try {
    await setUserPref(WEEK_START_PREF, String(value));
  } catch {
    // Ignore — the chosen start still applies for this session.
  }
}
