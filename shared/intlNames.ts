// Weekday and month names in the reader's language, from the platform.
//
// Both editors' repeat pickers spelled these out for themselves, and the
// repeat summary (84a) needs the same names in the same language on both
// surfaces. Nothing here is a rule — which day a rule names is decided in the
// core; this only says what that day is called.

import type { Weekday } from './generated/Weekday';

/** The core's weekdays, Monday first, as RFC 5545 counts a week. */
export const WEEKDAYS_FROM_MONDAY: Weekday[] = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday',
];

/** The `BYDAY` token of a weekday: `MO`, `TU`, … */
export function weekdayToken(day: Weekday): string {
  const tokens = ['MO', 'TU', 'WE', 'TH', 'FR', 'SA', 'SU'];
  return tokens[Math.max(0, WEEKDAYS_FROM_MONDAY.indexOf(day))];
}

/**
 * The name of a weekday in `language`. Falls back to the English name the
 * core uses, so a platform without the locale data still says a word.
 */
export function weekdayName(language: string, day: Weekday): string {
  const index = Math.max(0, WEEKDAYS_FROM_MONDAY.indexOf(day));
  // 2024-01-01 is a Monday, so the index offsets from there.
  const date = new Date(2024, 0, 1 + index);
  try {
    return new Intl.DateTimeFormat(language, { weekday: 'long' }).format(date);
  } catch {
    return day;
  }
}

/** The name of a month, 1 to 12, in `language`. */
export function monthName(language: string, month: number): string {
  const index = Math.min(11, Math.max(0, Math.round(month) - 1));
  try {
    return new Intl.DateTimeFormat(language, { month: 'long' }).format(
      new Date(2024, index, 1),
    );
  } catch {
    return String(index + 1);
  }
}

/**
 * A day (`YYYY-MM-DD`) written out in `language`: "15 March 2027", „15. März
 * 2027". Read in UTC, because a day key names a day and not an instant — the
 * desktop and the phone would otherwise write the same day differently.
 */
export function formatLongDay(dayKey: string, language: string): string {
  const date = new Date(`${dayKey}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) return dayKey;
  try {
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'long',
      timeZone: 'UTC',
    }).format(date);
  } catch {
    return dayKey;
  }
}
