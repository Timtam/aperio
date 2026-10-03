// What to say about the device's own calendars and reminders: after Aperio
// asked the OS (at start, decision 166, or from the account's "Allow
// access…"), after a grant given in the OS settings, and in the repair dialog.
//
// Whether to ask, and what the repair does, are the core's rules
// (`cal_core::os_access::ask_on_start`, `repair_for`); this only reads the
// answers back. One sentence follows a prompt, so it has to say what came of
// it as a whole: everything granted, only one of two, or nothing.

import type { AskFor } from './generated/AskFor';
import type { OsAccess } from './generated/OsAccess';
import type { OsAccessReport } from './generated/OsAccessReport';

export type { AccessRepair } from './generated/AccessRepair';
export type { AskFor } from './generated/AskFor';
export type { OsAccess } from './generated/OsAccess';
export type { OsAccessReport } from './generated/OsAccessReport';

/** What came of asking. */
export type AskOutcome = 'granted' | 'calendarsOnly' | 'remindersOnly' | 'denied';

/**
 * What came of asking for `asked`, read from the report taken afterwards.
 *
 * Only the entities that were asked about count. One the OS had already
 * granted was not part of the question, and naming it would credit the prompt
 * with something it did not do.
 */
export function askOutcome(asked: AskFor, after: OsAccessReport): AskOutcome {
  const events = asked.events && after.calendar === 'full';
  const reminders = asked.reminders && after.tasks === 'full';
  const all = (!asked.events || events) && (!asked.reminders || reminders);
  if (all) return 'granted';
  if (events) return 'calendarsOnly';
  if (reminders) return 'remindersOnly';
  return 'denied';
}

/** Whether anything that was asked about is now granted, so the device
 *  accounts are worth reading again. */
export function anyGranted(asked: AskFor, after: OsAccessReport): boolean {
  return askOutcome(asked, after) !== 'denied';
}

/**
 * What a grant made outside a prompt of Aperio's amounts to (the OS settings,
 * while Aperio waited or was closed), read from the report after it: every
 * store, or only one of two.
 *
 * Everything readable counts here, since nothing was asked: the question is
 * what Aperio can read again.
 */
export function grantedOutcome(report: OsAccessReport): AskOutcome {
  return askOutcome({ events: true, reminders: report.tasks != null }, report);
}

/**
 * What a prompt came to, said as what Aperio can read now: `denied` when
 * nothing asked about was granted, otherwise every store readable after it
 * ([`grantedOutcome`]). A store refused earlier, not part of the question,
 * is then still named as missing instead of "access granted".
 */
export function answerOutcome(asked: AskFor, after: OsAccessReport): AskOutcome {
  return askOutcome(asked, after) === 'denied' ? 'denied' : grantedOutcome(after);
}

/** Which of the device's stores a sentence is about. */
export type DeviceStores = 'none' | 'calendars' | 'reminders' | 'both';

/** The two stores' access, as a report or a remembered look carries it. */
export type DeviceAccessPair = Pick<OsAccessReport, 'calendar' | 'tasks'>;

function storesWhere(access: DeviceAccessPair, test: (a: OsAccess) => boolean): DeviceStores {
  const calendars = test(access.calendar);
  const reminders = access.tasks != null && test(access.tasks);
  if (calendars && reminders) return 'both';
  if (calendars) return 'calendars';
  if (reminders) return 'reminders';
  return 'none';
}

/** The stores Aperio may not read: for the account's badge and for "still
 *  missing". Add-only counts as missing (decision 171). */
export function missingStores(access: DeviceAccessPair): DeviceStores {
  return storesWhere(access, (a) => a !== 'full');
}

/** The stores the OS settings for Aperio can open: refused, add-only or
 *  undetermined. Not one a policy forbids (its own settings decide), and not
 *  one the OS still asks about (asking comes first). */
export function settingsStores(access: DeviceAccessPair): DeviceStores {
  return storesWhere(
    access,
    (a) => a !== 'full' && a !== 'restricted' && a !== 'not_asked',
  );
}

/** The stores a policy forbids (Screen Time, a device profile). */
export function restrictedStores(access: DeviceAccessPair): DeviceStores {
  return storesWhere(access, (a) => a === 'restricted');
}

/** Whether a store became readable between two looks. */
export function gainedAccess(before: DeviceAccessPair, after: DeviceAccessPair): boolean {
  return (
    (before.calendar !== 'full' && after.calendar === 'full') ||
    (before.tasks !== 'full' && after.tasks === 'full')
  );
}
