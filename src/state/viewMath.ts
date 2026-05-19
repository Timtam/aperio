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

export type ViewId = 'day' | 'week' | 'month' | 'year' | 'agenda' | 'tasks';

export const VIEWS: ViewId[] = [
  'day',
  'week',
  'month',
  'year',
  'agenda',
  'tasks',
];

/** Range visible in the given view, anchored at `date`. */
export function visibleRange(view: ViewId, date: Date): { start: Date; end: Date } {
  switch (view) {
    case 'day':
      return { start: startOfDay(date), end: endOfDay(date) };
    case 'week':
      return {
        start: startOfWeek(date, { weekStartsOn: 1 }),
        end: endOfWeek(date, { weekStartsOn: 1 }),
      };
    case 'month':
      return { start: startOfMonth(date), end: endOfMonth(date) };
    case 'year':
      return { start: startOfYear(date), end: endOfYear(date) };
    case 'agenda':
      // Agenda shows the next ~30 days from `date` by default.
      return { start: startOfDay(date), end: endOfDay(addDays(date, 30)) };
    case 'tasks':
      // Task view is not date-driven, but other parts of the store still
      // pull events to colour-code "tasks due today". Use a month range.
      return { start: startOfMonth(date), end: endOfMonth(date) };
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
  }
}

/** Today, anchored at midnight. */
export function today(): Date {
  return startOfDay(new Date());
}
