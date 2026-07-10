import { getUserPref, setUserPref } from '../api/prefs';

// "Show cancelled events" — SYNCED across the user's devices via the
// `view.showCancelledEvents` user-pref (a §19.2.1 always-sync key), the mobile
// twin of the desktop ViewState's `showCancelledEvents`. When ON (the default,
// for Outlook consistency) cancelled meetings stay visible in the calendar;
// when OFF they're hidden. Either way the host suppresses their reminders. The
// host reads this pref directly in `get_events_json`, so toggling it takes
// effect on the next calendar read; GeneralSettingsScreen is the toggle.

const SHOW_CANCELLED_PREF = 'view.showCancelledEvents';

/** Read the synced show-cancelled pref; defaults to true (show) when unset or
 *  on any value other than the literal "false". */
export async function readShowCancelledEvents(): Promise<boolean> {
  try {
    return (await getUserPref(SHOW_CANCELLED_PREF)) !== 'false';
  } catch {
    return true;
  }
}

/** Persist the pref to the synced key. Best-effort — a write failure leaves the
 *  caller's local toggle state reflecting the choice for this session. */
export async function writeShowCancelledEvents(value: boolean): Promise<void> {
  try {
    await setUserPref(SHOW_CANCELLED_PREF, value ? 'true' : 'false');
  } catch {
    // Ignore — the choice still applies on the next calendar read.
  }
}
