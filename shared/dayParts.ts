// The parts a repeat rule takes from the day a series starts on — this
// surface's door into `cal_core::day_parts`.
//
// A rule without BYDAY, BYMONTHDAY or BYMONTH repeats on the weekday, the day
// of the month and the month of its start, and the repeat controls offer "the
// 24th", "the fourth Tuesday" and, in a month's last seven days, "the last
// Tuesday". The controls read those off a `Date` on the device's clock; the
// core reads them off the day key the form holds, whichever clock its fields
// stand on (DESIGN-series-time-zone.md, stage 8). This file only asks.
//
// Pinned by `crates/cal-core/tests/fixtures/dayParts.json`, computed with
// Python's datetime; `dayParts.contract.test.ts` replays it through this door,
// and the core and the phone's bridge read the same file.

import type { DayParts } from './generated/DayParts';

export type { DayParts };

/** This surface's door into the core: one day key in, its parts as JSON. */
export interface DayPartsRules {
  dayPartsJson(dayKey: string): string;
}

let installedRules: DayPartsRules | null = null;

/** Bind this surface's door into the core. */
export function installDayPartsRules(rules: DayPartsRules): void {
  installedRules = rules;
}

/** The parts of the day `dayKey` (`YYYY-MM-DD`) names, or `null` for a key
 *  the core refuses (empty, `2026-02-30`): a form whose date field is being
 *  typed must not throw while it renders. */
export function dayParts(dayKey: string): DayParts | null {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'day parts used before installDayPartsRules() — the surface must bind its ' +
        'door into cal_core::day_parts at startup',
    );
  }
  let answer: string;
  try {
    answer = installedRules.dayPartsJson(dayKey);
  } catch {
    return null;
  }
  return JSON.parse(answer) as DayParts;
}
