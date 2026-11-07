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
 * "After start" (TODO B7 222) is a relative reminder with negative minutes,
 * a positive `TRIGGER` in iCalendar.
 *
 * An all-day appointment's rows read differently ({@link AllDayReminder}): a
 * day and a clock time, the reading the planner gives the appointment's own
 * reminders (`AllDayReading::Exact` in `crates/host-core/src/reminders.rs`).
 */

import type { ReminderKind } from './types';

/** What the kind field of a reminder row offers. */
export type ReminderKindOption = 'relative' | 'at_start' | 'after_start' | 'absolute' | 'app_start';

export type RelativeUnit = 'minutes' | 'hours' | 'days';

export const RELATIVE_UNIT_MINUTES: Record<RelativeUnit, number> = {
  minutes: 1,
  hours: 60,
  days: 60 * 24,
};

/** What a reminder newly set to "before start" starts as. */
export const DEFAULT_MINUTES_BEFORE = 15;

/** What a reminder newly set to "after start" starts as: 15 minutes after. */
export const DEFAULT_MINUTES_AFTER = 15;

const MINUTES_PER_DAY = 24 * 60;

/**
 * The option a stored reminder shows as. Zero minutes before is "at start",
 * fewer than zero "after start"; an e-mail reminder (adapter-side, not edited
 * here) shows as a relative one, as it always has.
 */
export function reminderKindOption(kind: ReminderKind): ReminderKindOption {
  switch (kind.type) {
    case 'relative':
      if (kind.minutes_before === 0) return 'at_start';
      return kind.minutes_before < 0 ? 'after_start' : 'relative';
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
    case 'after_start':
      return { type: 'relative', minutes_before: -DEFAULT_MINUTES_AFTER };
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

/** The words for "after start": after the start, or after a task is due. */
export function afterStartLabelKey(mode: 'event' | 'task'): string {
  return mode === 'task' ? 'reminders.kind.afterDue' : 'reminders.kind.afterStart';
}

/**
 * Whether a row may be "after start" — on an all-day appointment, any reminder
 * at or after its day's midnight but "on the day" at the day-start time. Only
 * where it arrives (decided 2026-11-07, TODO B7 222): the calendar keeps it
 * (`CalendarRow.reminder_capabilities.after_start`: CalDAV, iCloud, the local
 * store, a birthday calendar), or the row stays in Aperio. Google, Exchange and
 * Outlook would leave it out.
 */
export function afterStartOffered(
  row: { attach?: boolean },
  target: { keptByCalendar: boolean; placement: boolean },
): boolean {
  // A row without the flag is a default stored before the placement choice
  // existed; it stays in Aperio and the editor shows it so.
  return target.keptByCalendar || (target.placement && row.attach !== true);
}

/** Whether a reminder rings after the start (or, all-day, after its day's
 *  midnight): the kind {@link afterStartOffered} rules on. */
export function isAfterStart(kind: ReminderKind): boolean {
  return kind.type === 'relative' && kind.minutes_before < 0;
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

/**
 * An all-day appointment's reminder as its editor shows it (decided
 * 2026-11-07, TODO B7 222): a day — the day itself, N days before, N days
 * after — and a clock time, either the day-start time or one of its own ("On
 * day of event, 09:00", as the iPhone sets it). It is the reading the planner
 * gives the appointment's own reminders: whole days ring at the day-start
 * time, anything else counts from local 00:00 of its day, as RFC 5545, the
 * iPhone and Google count it.
 */
export interface AllDayReminder {
  /** 0 the day itself, below 0 that many days before, above 0 after. */
  day: number;
  /** Minutes after midnight, 1 to 1439; `null` for the day-start time. */
  clock: number | null;
}

/** The day and clock time an all-day appointment's own reminder rings at. */
export function allDayReminderOf(minutesBefore: number): AllDayReminder {
  if (minutesBefore % MINUTES_PER_DAY === 0) {
    return { day: -minutesBefore / MINUTES_PER_DAY || 0, clock: null };
  }
  const afterMidnight = -minutesBefore;
  const day = Math.floor(afterMidnight / MINUTES_PER_DAY);
  return { day, clock: afterMidnight - day * MINUTES_PER_DAY };
}

/** `minutes_before` for an all-day reminder. A clock time of 00:00 is the
 *  whole day, which rings at the day-start time. */
export function minutesBeforeOfAllDay({ day, clock }: AllDayReminder): number {
  return -(day * MINUTES_PER_DAY + (clock ?? 0)) || 0;
}

/** The clock time an all-day reminder newly set to one starts at: 09:00, the
 *  iPhone's time for its all-day alerts. */
export const DEFAULT_ALL_DAY_CLOCK = 9 * 60;

/**
 * A calendar default for an all-day appointment, as the planner rings it and
 * as `apply_default_reminder_policy` writes it into a new one: whole days,
 * rounded to nearest (12 h → 1 day, half away from zero), so "15 minutes
 * before" is the day itself. The editors pre-fill an all-day appointment with
 * the defaults in this form, so what they show is what is saved.
 */
export function allDayDefaultMinutes(minutesBefore: number): number {
  const days = minutesBefore / MINUTES_PER_DAY;
  const whole = Math.sign(days) * Math.round(Math.abs(days));
  return whole * MINUTES_PER_DAY || 0;
}

/** "HH:MM" for minutes after midnight. */
export function clockText(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
}

/** Minutes after midnight for "H:MM" or "HH:MM" — or with a dot, "9.30", as
 *  German writes it — or `null` when it is no time of day. */
export function clockOfText(text: string): number | null {
  const match = /^(\d{1,2})[:.](\d{2})$/.exec(text.trim());
  if (!match) return null;
  const h = Number(match[1]);
  const m = Number(match[2]);
  return h < 24 && m < 60 ? h * 60 + m : null;
}

/** What the kind field of an all-day appointment's reminder row offers. */
export type AllDayKindOption = 'on_day' | 'days_before' | 'days_after' | 'absolute' | 'app_start';

/** The option an all-day appointment's reminder shows as; an e-mail reminder
 *  (adapter-side, not edited here) shows as "on the day". */
export function allDayKindOption(kind: ReminderKind): AllDayKindOption {
  switch (kind.type) {
    case 'relative': {
      const { day } = allDayReminderOf(kind.minutes_before);
      if (day === 0) return 'on_day';
      return day < 0 ? 'days_before' : 'days_after';
    }
    case 'absolute':
      return 'absolute';
    case 'app_start':
      return 'app_start';
    case 'email':
      return 'on_day';
  }
}

/** The reminder an all-day row becomes when its kind is chosen: the day
 *  itself, one day before or one day after, at the day-start time. */
export function allDayKindFor(option: AllDayKindOption, now: Date): ReminderKind {
  switch (option) {
    case 'on_day':
      return { type: 'relative', minutes_before: 0 };
    case 'days_before':
      return { type: 'relative', minutes_before: MINUTES_PER_DAY };
    case 'days_after':
      return { type: 'relative', minutes_before: -MINUTES_PER_DAY };
    case 'absolute':
    case 'app_start':
      return reminderKindFor(option, now);
  }
}

/** A calendar default's kind as an all-day appointment takes it
 *  ({@link allDayDefaultMinutes}); every other kind as it is. */
export function allDayDefaultKind(kind: ReminderKind): ReminderKind {
  return kind.type === 'relative'
    ? { type: 'relative', minutes_before: allDayDefaultMinutes(kind.minutes_before) }
    : kind;
}
