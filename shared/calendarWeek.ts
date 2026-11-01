// The calendar week a day is shown in — this surface's door into
// `cal_core::calendar_week` (DESIGN §5.2).
//
// The core decides the number: ISO 8601 in every language, the ISO week the
// fourth day of the displayed week falls in, so a week that starts on Sunday
// or Saturday still carries one number. It also says where the week begins
// and ends and how its range is written (one month, or two years). This file
// only asks; a second rule here would be the copy the core exists to
// replace.
//
// Pinned by `crates/cal-core/tests/fixtures/calendarWeek.json`, computed
// independently of both; `calendarWeek.contract.test.ts` replays it through
// this door, and the core's own contract test reads the same file.

import type { CalendarWeek } from './generated/CalendarWeek';
import type { CalendarWeeksInput } from './generated/CalendarWeeksInput';
import type { DayCalendarWeek } from './generated/DayCalendarWeek';

export type { CalendarWeek, CalendarWeeksInput, DayCalendarWeek };

/** This surface's door into the core: one question, one answer. */
export interface CalendarWeekRules {
  calendarWeeksJson(inputJson: string): string;
}

let installedRules: CalendarWeekRules | null = null;

/** Bind this surface's door into the core. */
export function installCalendarWeekRules(rules: CalendarWeekRules): void {
  installedRules = rules;
}

/**
 * The calendar week each of `dayKeys` (`YYYY-MM-DD`, local days) is shown in,
 * in their order: one call for a month grid or a list. `weekStartsOn` is the
 * user's setting, 0 = Sunday … 6 = Saturday.
 */
export function calendarWeeks(dayKeys: string[], weekStartsOn: number): DayCalendarWeek[] {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'calendar week rules used before installCalendarWeekRules() — the surface ' +
        'must bind its door into cal_core::calendar_week at startup',
    );
  }
  const input: CalendarWeeksInput = {
    days: dayKeys,
    weekStartsOn: ((weekStartsOn % 7) + 7) % 7,
  };
  return JSON.parse(installedRules.calendarWeeksJson(JSON.stringify(input))) as DayCalendarWeek[];
}

/** The calendar week one day is shown in. */
export function calendarWeekOf(dayKey: string, weekStartsOn: number): CalendarWeek {
  const [answer] = calendarWeeks([dayKey], weekStartsOn);
  if (answer === undefined) throw new Error('the core answered no week for one day');
  return answer.week;
}
