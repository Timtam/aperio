// Shifting a recurring series by whole days — this surface's door into
// `cal_core::series_shift`.
//
// Dragging a whole series onto another day moves every occurrence by the same
// number of days: the rule's weekdays and day of the month move with it, and so
// does its UNTIL. That rewrite is a rule and lives in the core. Days are counted
// on the series' own clock (its zone, or UTC without one), where its rule is
// read. The shell moves what needs the zone: the start, the exceptions, and a
// UTC UNTIL, which it hands to the core already moved. A rule that cannot move
// by whole days (the second Sunday of a month, a set position, a day past the
// 28th, …) comes back refused, and the surface offers to move only the
// occurrence instead.

import type { SeriesShift, SeriesShiftQuestion, ShiftRefusal } from './types';

export type { SeriesShift, ShiftRefusal };

/** This surface's door into `cal_core::series_shift`: one question, one answer. */
export interface SeriesShiftRules {
  seriesShiftJson(inputJson: string): string;
}

let installedRules: SeriesShiftRules | null = null;

/** Bind this surface's door into the core. */
export function installSeriesShiftRules(rules: SeriesShiftRules): void {
  installedRules = rules;
}

/**
 * The rule for a series that starts on `startDayKey` (`YYYY-MM-DD` on the
 * series' own clock) and moves by `days` whole days on that clock, with the
 * time of day changing too when `timeChanges`. `until` is the rule's UTC UNTIL
 * already moved on the series' clock (`YYYYMMDDTHHMMSSZ`), when it has one.
 */
export function shiftSeriesRule(
  rrule: string,
  startDayKey: string,
  days: number,
  timeChanges: boolean,
  until?: string,
): SeriesShift {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'series shift rules used before installSeriesShiftRules() — the surface ' +
        'must bind its door into cal_core::series_shift at startup',
    );
  }
  const question: SeriesShiftQuestion = {
    rrule,
    start: startDayKey,
    days,
    time_changes: timeChanges,
    until: until ?? null,
  };
  return JSON.parse(installedRules.seriesShiftJson(JSON.stringify(question))) as SeriesShift;
}
