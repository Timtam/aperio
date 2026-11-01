import { getUserPref, setUserPref } from '../api/prefs';

// "Show and announce calendar weeks" — SYNCED across the user's devices via
// the `view.weekNumbers` user-pref (a §19.2.1 always-sync key), the mobile twin
// of the desktop ViewState's `weekNumbers`. When ON (the default) the week
// view's heading names the week, seen as "KW 45" and spoken as "Kalenderwoche
// 45", the month list names it on the first day of each week, and the day
// view's announcement on a week change; when OFF none of them does.
// GeneralSettingsScreen is the toggle; WeekScreen, MonthScreen and
// EventsScreen read it.
//
// The last value read or written is kept here, and the app reads it once at
// launch: a screen then starts on the stored choice instead of the default,
// and a heading that changed a moment after opening would be announced by the
// VoiceOver pager as if the user had paged.

const WEEK_NUMBERS_PREF = 'view.weekNumbers';

let known: boolean | null = null;

/** The last known value, without waiting: the stored one once read, else the
 *  default (on). */
export function peekWeekNumbers(): boolean {
  return known ?? true;
}

/** Read the synced pref; defaults to true (on) when unset or on any value
 *  other than the literal "false". */
export async function readWeekNumbers(): Promise<boolean> {
  try {
    known = (await getUserPref(WEEK_NUMBERS_PREF)) !== 'false';
  } catch {
    return peekWeekNumbers();
  }
  return known;
}

/** Persist the pref to the synced key. Best-effort — a write failure leaves the
 *  caller's local toggle state reflecting the choice for this session. */
export async function writeWeekNumbers(value: boolean): Promise<void> {
  known = value;
  try {
    await setUserPref(WEEK_NUMBERS_PREF, value ? 'true' : 'false');
  } catch {
    // Ignore — the choice still applies for this session.
  }
}
