/**
 * The kinds a reminder row offers (DESIGN.md §14.1), and how a stored
 * reminder reads as one of them. Both reminder editors, desktop and phone,
 * decide by these, so a reminder reads the same on both and a row nobody
 * touched is saved as it came.
 *
 * "At start" is no stored kind of its own: it is a relative reminder of zero
 * minutes, which is what every provider writes for it (RFC 5545
 * `TRIGGER:PT0S`, Google and Outlook 0 minutes before). Offering it as its own
 * kind is what lets an editor show it as what it is instead of "0 minutes
 * before", and keep it: an amount field only takes whole numbers from 1.
 */

import type { ReminderKind } from './types';

/** What the kind field of a reminder row offers. */
export type ReminderKindOption = 'relative' | 'at_start' | 'absolute' | 'app_start';

export type RelativeUnit = 'minutes' | 'hours' | 'days';

export const RELATIVE_UNIT_MINUTES: Record<RelativeUnit, number> = {
  minutes: 1,
  hours: 60,
  days: 60 * 24,
};

/** What a reminder newly set to "before start" starts as. */
export const DEFAULT_MINUTES_BEFORE = 15;

/**
 * The option a stored reminder shows as. Zero minutes before is "at start";
 * an e-mail reminder (adapter-side, not edited here) shows as a relative one,
 * as it always has.
 */
export function reminderKindOption(kind: ReminderKind): ReminderKindOption {
  switch (kind.type) {
    case 'relative':
      return kind.minutes_before === 0 ? 'at_start' : 'relative';
    case 'absolute':
      return 'absolute';
    case 'app_start':
      return 'app_start';
    case 'email':
      return 'relative';
  }
}

/**
 * The reminder a row becomes when its kind is chosen. Back to "before start"
 * gives 15 minutes, whatever the row was; a fixed time starts tomorrow at
 * this hour, on the hour, far enough off that it is unlikely to stay by
 * accident.
 */
export function reminderKindFor(option: ReminderKindOption, now: Date): ReminderKind {
  switch (option) {
    case 'relative':
      return { type: 'relative', minutes_before: DEFAULT_MINUTES_BEFORE };
    case 'at_start':
      return { type: 'relative', minutes_before: 0 };
    case 'absolute': {
      const at = new Date(now.getTime());
      at.setMinutes(0, 0, 0);
      at.setDate(at.getDate() + 1);
      return { type: 'absolute', at: at.toISOString() };
    }
    case 'app_start':
      return { type: 'app_start' };
  }
}

/**
 * The words for "at start" where the row stands. A task rings when it is due
 * (at nine when it has no time); an all-day appointment has no start time and
 * rings on its day, at the time the day starts in Aperio (00:00 unless set).
 */
export function atStartLabelKey(mode: 'event' | 'task', allDay: boolean): string {
  if (mode === 'task') return 'reminders.kind.atDue';
  return allDay ? 'reminders.kind.onTheDay' : 'reminders.kind.atStart';
}

/** `minutes_before` as the largest whole unit and its amount. */
export function splitRelativeMinutes(minutes: number): { amount: number; unit: RelativeUnit } {
  if (minutes <= 0) return { amount: 0, unit: 'minutes' };
  if (minutes % RELATIVE_UNIT_MINUTES.days === 0) {
    return { amount: minutes / RELATIVE_UNIT_MINUTES.days, unit: 'days' };
  }
  if (minutes % RELATIVE_UNIT_MINUTES.hours === 0) {
    return { amount: minutes / RELATIVE_UNIT_MINUTES.hours, unit: 'hours' };
  }
  return { amount: minutes, unit: 'minutes' };
}

/**
 * What an amount typed into "before start" counts as: whole numbers from 1.
 * Cut to a whole number first, then checked, so a pasted 0.5 or a comma
 * before the first digit is never zero minutes before — that would be "at
 * start", a different kind the row did not choose.
 */
export function relativeAmount(typed: number): number {
  const whole = Math.trunc(typed);
  return Number.isFinite(whole) && whole >= 1 ? whole : 1;
}
