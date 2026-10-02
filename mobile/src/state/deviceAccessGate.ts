import { AccessibilityInfo, Platform } from 'react-native';

import { askOutcome, type AskOutcome } from '@aperio/shared';

import i18n from '../../i18n';
import { deviceCalendarAccess, requestDeviceCalendarAccess } from '../api/accounts';
import { refreshExternalCache } from '../api/sync';
import { holdingSpeech } from '../a11y/speechHold';
import { whileOsSheetOpen } from './appLock';
import { settleExternalCaches } from './cacheSettle';

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

/** How long the day-start review waits for the check at most. Long enough to
 *  answer an alert, reload and hear the sentence; a check that never settles
 *  must not cost the morning review. */
const SETTLE_CAP_MS = 120_000;
/** After an OS alert the system's own screen-change speech comes first, and a
 *  sentence spoken at once is cut off (the AppLockGate pattern). */
const ANNOUNCE_DELAY_MS = 400;
/** The longest of the sentences takes a few seconds to say; past this, stop
 *  waiting for VoiceOver to report it finished. */
const SPEAK_CAP_MS = 15_000;

let started = false;
let settle: () => void = () => {};
const settled = new Promise<void>((resolve) => {
  settle = resolve;
});

/** Resolves once the start check has finished — asked, reloaded and said, or
 *  found nothing to ask — or after a cap, whichever comes first. The day-start
 *  review waits for it, so it neither opens under the alert, nor judges the
 *  frozen cache a grant is about to replace, nor cuts the sentence off with
 *  its own "checking". */
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

/**
 * Say `message` and resolve once VoiceOver has finished it (iOS reports that),
 * at once without a screen reader, or after a cap. Queued, so it follows
 * whatever is being said rather than cutting it off.
 */
async function sayAndWait(message: string): Promise<void> {
  const screenReader = await AccessibilityInfo.isScreenReaderEnabled().catch(() => false);
  await new Promise((resolve) => setTimeout(resolve, ANNOUNCE_DELAY_MS));
  if (!screenReader) {
    AccessibilityInfo.announceForAccessibility(message);
    return;
  }
  await new Promise<void>((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      clearTimeout(cap);
      subscription?.remove();
      resolve();
    };
    const subscription =
      Platform.OS === 'ios'
        ? AccessibilityInfo.addEventListener('announcementFinished', (event) => {
            if (event.announcement === message) finish();
          })
        : null;
    const cap = setTimeout(finish, SPEAK_CAP_MS);
    AccessibilityInfo.announceForAccessibilityWithOptions(message, { queue: true });
  });
}

/** Run the start check once. Later calls wait for the first. */
export async function runDeviceAccessStartCheck(): Promise<void> {
  if (started) return settled;
  started = true;
  try {
    const before = await deviceCalendarAccess();
    const ask = before.ask_now;
    if (ask == null) return;
    // The alert flips the app inactive; the lock must not cover the app or
    // start its own prompt over it.
    await whileOsSheetOpen(() =>
      requestDeviceCalendarAccess(ask.events, ask.reminders),
    ).catch(() => false);
    const after = await deviceCalendarAccess();
    const outcome = askOutcome(ask, after);
    const reload = outcome !== 'denied';
    // Kick the reload first, so "… wird aktualisiert" is true while it is
    // said; the refresh cues queue behind the sentence instead of cutting it.
    if (reload) await refreshExternalCache().catch(() => {});
    const name = after.account_names[0] ?? before.account_names[0] ?? '';
    await holdingSpeech(() => sayAndWait(sentence(outcome, name)));
    // The day-start review reads the account next: wait for that reload.
    if (reload) await settleExternalCaches(async () => {});
  } catch {
    // Nothing was decided; the next start asks again.
  } finally {
    settle();
  }
}
