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
import { monthName } from './intlNames';

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

// ── How a week is written ───────────────────────────────────────────────────
//
// Both devices say a week the same way, so the words are built here once from
// the core's answer. Spoken: "Kalenderwoche 45" — never "KW 45", which a
// speech synthesizer spells out letter by letter. Seen: "KW 45". The year
// joins only when the week touches two calendar years, and then it is the
// year the number counts in. A range always says "bis", never a dash, which
// the synthesizer reads as "minus" or skips; and no number is followed by a
// full stop, which it would read as an ordinal.

type Translate = (key: string, vars?: Record<string, unknown>) => string;

/** Year, month and day of a `YYYY-MM-DD` key. */
function partsOf(dayKey: string): { year: number; month: number; day: number } {
  const [year, month, day] = dayKey.split('-').map(Number);
  return { year, month, day };
}

/** The week as it is spoken: "Kalenderwoche 45", or "Kalenderwoche 53 des
 *  Jahres 2026" when the week touches two calendar years. */
export function calendarWeekName(t: Translate, week: CalendarWeek): string {
  return week.spansYears
    ? t('views.week.kwSpokenYear', { week: week.week, year: week.weekYear })
    : t('views.week.kwSpoken', { week: week.week });
}

/** The week as it is seen: "KW 45", or "KW 53 (2026)". */
export function calendarWeekLabel(t: Translate, week: CalendarWeek): string {
  return week.spansYears
    ? t('views.week.kwYear', { week: week.week, year: week.weekYear })
    : t('views.week.kw', { week: week.week });
}

/** The week's days, first to last: "2. bis 8. November 2026", "26. Oktober
 *  bis 1. November 2026", "28. Dezember 2026 bis 3. Januar 2027". */
export function calendarWeekRange(t: Translate, language: string, week: CalendarWeek): string {
  const from = partsOf(week.firstDay);
  const to = partsOf(week.lastDay);
  if (week.sameMonth) {
    return t('views.week.rangeSameMonth', {
      fromDay: from.day,
      toDay: to.day,
      month: monthName(language, to.month),
      year: to.year,
    });
  }
  if (!week.spansYears) {
    return t('views.week.rangeSameYear', {
      fromDay: from.day,
      fromMonth: monthName(language, from.month),
      toDay: to.day,
      toMonth: monthName(language, to.month),
      year: to.year,
    });
  }
  return t('views.week.rangeYears', {
    fromDay: from.day,
    fromMonth: monthName(language, from.month),
    fromYear: from.year,
    toDay: to.day,
    toMonth: monthName(language, to.month),
    toYear: to.year,
  });
}

/** The whole week in one sentence, what the K key and a grid's name say:
 *  "Kalenderwoche 45, 2. bis 8. November 2026". */
export function calendarWeekSentence(t: Translate, language: string, week: CalendarWeek): string {
  return t('views.week.kwSentence', {
    name: calendarWeekName(t, week),
    range: calendarWeekRange(t, language, week),
  });
}
