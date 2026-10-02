// What to say after Aperio asked the OS for the device's own calendars and
// reminders at start (decision 166).
//
// Whether to ask is the core's rule (`cal_core::os_access::ask_on_start`); this
// only reads the answer back. One sentence follows the prompt, so it has to
// say what came of it as a whole: everything granted, only one of two, or
// nothing.

import type { AskFor } from './generated/AskFor';
import type { OsAccessReport } from './generated/OsAccessReport';

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
