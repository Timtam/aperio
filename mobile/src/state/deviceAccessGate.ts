import { AccessibilityInfo } from 'react-native';

import { askOutcome, type AskOutcome } from '@aperio/shared';

import i18n from '../../i18n';
import { deviceCalendarAccess, requestDeviceCalendarAccess } from '../api/accounts';
import { refreshExternalCache } from '../api/sync';
import { whileOsSheetOpen } from './appLock';

/**
 * At start, ask the OS for the device's own calendars and reminders when it
 * has never asked on this phone and a "this device" account exists
 * (decision 166).
 *
 * A phone Aperio was moved to brings the account and its cache along, but not
 * the grant: iOS never asked there, every read of the account failed, and its
 * cache froze. Whether to ask is the core's rule (`device_calendar_access_json`
 * carries its answer); this runs the prompt, reads the answer back, reloads the
 * account when something was granted, and says one sentence about it.
 *
 * Once per process. Nothing else asks: an answer the user already gave is
 * theirs, and iOS would not show the prompt again anyway.
 */

/** How long the day-start review waits for the prompt at most. Long enough to
 *  read and answer an alert; a check that never settles must not cost the
 *  morning review. */
const SETTLE_CAP_MS = 120_000;
/** After an OS alert the system's own screen-change speech comes first, and a
 *  sentence spoken at once is cut off (the AppLockGate pattern). */
const ANNOUNCE_DELAY_MS = 400;

let started = false;
let settle: () => void = () => {};
const settled = new Promise<void>((resolve) => {
  settle = resolve;
});

/** Resolves once the start check has finished — asked and answered, or found
 *  nothing to ask — or after a cap, whichever comes first. The day-start review
 *  waits for it, so it neither opens under the alert nor judges the frozen
 *  cache the grant is about to replace. */
export function whenDeviceAccessSettled(): Promise<void> {
  return Promise.race([
    settled,
    new Promise<void>((resolve) => setTimeout(resolve, SETTLE_CAP_MS)),
  ]);
}

function sentence(outcome: AskOutcome, name: string): string {
  switch (outcome) {
    case 'granted':
      return i18n.t('mobile.deviceAccess.granted', { name });
    case 'calendarsOnly':
      return i18n.t('mobile.deviceAccess.calendarsOnly', { name });
    case 'remindersOnly':
      return i18n.t('mobile.deviceAccess.remindersOnly', { name });
    case 'denied':
      return i18n.t('dialogs.accounts.deviceAccessDenied');
  }
}

/** Run the start check once. Later calls wait for the first. */
export async function runDeviceAccessStartCheck(): Promise<void> {
  if (started) return settled;
  started = true;
  try {
    const before = await deviceCalendarAccess();
    const ask = before.ask_now;
    if (ask == null) return;
    // The alert flips the app inactive; the lock must not cover or re-lock
    // the app under it.
    await whileOsSheetOpen(() =>
      requestDeviceCalendarAccess(ask.events, ask.reminders),
    ).catch(() => false);
    const after = await deviceCalendarAccess();
    const outcome = askOutcome(ask, after);
    // Reload first, so its "updating" cue is not what cuts the sentence off.
    if (outcome !== 'denied') await refreshExternalCache().catch(() => {});
    const name = after.account_names[0] ?? before.account_names[0] ?? '';
    const message = sentence(outcome, name);
    setTimeout(() => AccessibilityInfo.announceForAccessibility(message), ANNOUNCE_DELAY_MS);
  } catch {
    // Nothing was decided; the next start asks again.
  } finally {
    settle();
  }
}
