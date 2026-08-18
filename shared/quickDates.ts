/** The handful of dates people actually pick.
 *
 *  Typing a date means first working out what today is, then reaching the
 *  field, then walking to the right day — for something as ordinary as
 *  "tomorrow". Every task app therefore offers the same short list, and this is
 *  it: today, tomorrow, the coming weekend, the start of next week.
 *
 *  Deliberately NOT configurable. A list this small is worth more as a fixed,
 *  learnable set than as one more thing to set up; the full date field is right
 *  there for everything else.
 *
 *  Pure and shared so the desktop and the phone offer the same four days and
 *  compute them the same way — a quick pick that disagreed between devices
 *  would be worse than none. */

import { localDateKey } from './dateKey';

export type QuickDateId = 'today' | 'tomorrow' | 'weekend' | 'nextWeek';

export interface QuickDate {
  id: QuickDateId;
  /** Local `YYYY-MM-DD`, ready for a date input. */
  dayKey: string;
}

/** Saturday, in `Date.getDay()` terms. */
const SATURDAY = 6;

function addDays(from: Date, days: number): Date {
  const d = new Date(from.getFullYear(), from.getMonth(), from.getDate());
  d.setDate(d.getDate() + days);
  return d;
}

/**
 * The four offers, relative to `today`.
 *
 * `weekStartsOn` is the user's own setting, so "next week" lands on the day
 * their week actually begins rather than on a hard-coded Monday.
 *
 * The weekend is the COMING Saturday — today when today is a Saturday, and six
 * days out on a Sunday, which is what "next weekend" means to somebody saying
 * it on a Sunday evening.
 */
export function quickDates(today: Date, weekStartsOn = 1): QuickDate[] {
  const base = new Date(today.getFullYear(), today.getMonth(), today.getDate());

  const untilSaturday = (SATURDAY - base.getDay() + 7) % 7;
  const weekend = addDays(base, untilSaturday);

  // Days from today to the next start-of-week. `% 7 || 7` keeps it strictly in
  // the future: standing ON the start of the week, "next week" is seven days
  // away, not today.
  const untilWeekStart = ((weekStartsOn - base.getDay() + 7) % 7) || 7;
  const nextWeek = addDays(base, untilWeekStart);

  return [
    { id: 'today', dayKey: localDateKey(base) },
    { id: 'tomorrow', dayKey: localDateKey(addDays(base, 1)) },
    { id: 'weekend', dayKey: localDateKey(weekend) },
    { id: 'nextWeek', dayKey: localDateKey(nextWeek) },
  ];
}
