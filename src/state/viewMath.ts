import {
  addDays,
  addMonths,
  addYears,
  endOfDay,
  endOfMonth,
  endOfWeek,
  endOfYear,
  startOfDay,
  startOfMonth,
  startOfWeek,
  startOfYear,
} from 'date-fns';

/**
 * View math: the date arithmetic each calendar view shares.
 *
 * Kept out of any React component so it can be tested as pure functions.
 *
 * Conventions:
 *  - Week views are ISO-8601 (Monday-start) — DESIGN.md section 5.2.
 *  - "Period" for shortcut navigation (Ctrl+Left/Right) is one day for
 *    DayView, one week for WeekView, one month for MonthView, one year
 *    for YearView, one month for AgendaView, and one month for TaskView.
 */

export type ViewId =
  | 'day'
  | 'week'
  | 'month'
  | 'year'
  | 'agenda'
  | 'tasks'
  | 'contacts';

/**
 * Which weekday a week visually starts on, as `date-fns`' `weekStartsOn`
 * (0 = Sunday … 6 = Saturday). Configurable per user (DESIGN.md §5.2,
 * synced pref `view.weekStart`). NOTE: this only affects the visual
 * column order — KW numbers stay ISO-8601 (Monday-based) regardless.
 */
export type WeekStart = 0 | 1 | 2 | 3 | 4 | 5 | 6;

export const VIEWS: ViewId[] = [
  'day',
  'week',
  'month',
  'year',
  'agenda',
  'tasks',
  'contacts',
];

/** How far one press of a time field's minute spinner moves.
 *
 *  Minute-by-minute is the browser default, and it is a lot of presses for a
 *  half-past-nine meeting — Outlook steps in 5, Google in 15. `1` keeps the
 *  old behaviour for anyone who wants it. */
export const TIME_STEP_CHOICES = [1, 5, 10, 15, 30] as const;
export type TimeStepMinutes = (typeof TIME_STEP_CHOICES)[number];
export const DEFAULT_TIME_STEP: TimeStepMinutes = 15;

export function isValidTimeStep(n: number): n is TimeStepMinutes {
  return (TIME_STEP_CHOICES as readonly number[]).includes(n);
}

/** Range visible in the given view, anchored at `date`. The week range
 *  honours the configurable `weekStartsOn` (defaults to Monday/ISO). */
export function visibleRange(
  view: ViewId,
  date: Date,
  weekStartsOn: WeekStart = 1,
): { start: Date; end: Date } {
  switch (view) {
    case 'day':
      return { start: startOfDay(date), end: endOfDay(date) };
    case 'week':
      return {
        start: startOfWeek(date, { weekStartsOn }),
        end: endOfWeek(date, { weekStartsOn }),
      };
    case 'month':
      // The GRID, not the month. The month view draws whole weeks, so it
      // renders up to six days of the previous month and six of the next —
      // and it used to fetch only the month itself, leaving those padding
      // days permanently empty. An event on the 31st was invisible to anyone
      // looking at the following month's first row.
      return {
        start: startOfWeek(startOfMonth(date), { weekStartsOn }),
        end: endOfWeek(endOfMonth(date), { weekStartsOn }),
      };
    case 'year':
      return { start: startOfYear(date), end: endOfYear(date) };
    case 'agenda':
      // Agenda shows the next ~30 days from `date` by default.
      return { start: startOfDay(date), end: endOfDay(addDays(date, 30)) };
    case 'tasks':
      // Task view is not date-driven, but other parts of the store still
      // pull events to colour-code "tasks due today". Use a month range.
      return { start: startOfMonth(date), end: endOfMonth(date) };
    case 'contacts':
      // Contacts view is date-less. Returning today as both endpoints
      // means no event range is computed for it, while keeping the
      // shape callers expect.
      return { start: startOfDay(date), end: endOfDay(date) };
  }
}

/** Step forward one period in the active view. */
export function nextPeriod(view: ViewId, date: Date): Date {
  switch (view) {
    case 'day':
      return addDays(date, 1);
    case 'week':
      return addDays(date, 7);
    case 'month':
    case 'agenda':
    case 'tasks':
      return addMonths(date, 1);
    case 'year':
      return addYears(date, 1);
    case 'contacts':
      // Contacts isn't time-anchored — Ctrl+Right is a no-op.
      return date;
  }
}

/** Step back one period in the active view. */
export function prevPeriod(view: ViewId, date: Date): Date {
  switch (view) {
    case 'day':
      return addDays(date, -1);
    case 'week':
      return addDays(date, -7);
    case 'month':
    case 'agenda':
    case 'tasks':
      return addMonths(date, -1);
    case 'year':
      return addYears(date, -1);
    case 'contacts':
      return date;
  }
}

/** Today, anchored at midnight. */
export function today(): Date {
  return startOfDay(new Date());
}
