/** localStorage key for the calendar id the user most recently created
 *  an event on. Read at form-open time so the dropdown defaults to it
 *  instead of `calendars[0]`, which is a usability win for anyone who
 *  has more than two calendars wired up. Scope is per-app-install;
 *  multi-profile / multi-device sync is out of scope here. */
const LAST_USED_CALENDAR_KEY = 'aperio.lastUsedCalendar.v1';

export function readLastUsedCalendar(): string | null {
  try {
    return localStorage.getItem(LAST_USED_CALENDAR_KEY);
  } catch {
    // localStorage can throw in private-browsing or quota-exceeded
    // states. The user just gets the regular `calendars[0]` fallback.
    return null;
  }
}

export function writeLastUsedCalendar(id: string): void {
  try {
    localStorage.setItem(LAST_USED_CALENDAR_KEY, id);
  } catch {
    // Best effort — no recovery needed.
  }
}
