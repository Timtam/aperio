// Dates and times on the clock a series repeats on — this surface's door into
// `cal_core::wall_clock` (DESIGN-series-time-zone.md, stage 8, decision 100a).
//
// A series' start and end are typed and shown on its own clock: the zone its
// stored name names, or UTC. The core turns a date and a time of day into an
// instant and back with the tzdata the adapters write with: a time the clock
// skips (the spring gap) has no instant and says so, a time it shows twice
// (the autumn fold) takes the earlier reading, a changed repeat end is
// 23:59:59 of its date on the clock, and a stored one reads through a date or
// as an instant (`seriesEnd`). The desktop asks through its WebAssembly
// module, which carries the zones; the phone through its native core. Both
// answer synchronously, so a form can ask while it renders.
//
// Pinned by `crates/cal-core/tests/fixtures/wallClock.json`;
// `wallClock.contract.test.ts` replays it through this door, and the core and
// the phone's bridge read the same file.

import type { EndReading } from './generated/EndReading';
import type { SeriesEnd } from './generated/SeriesEnd';
import type { WallClockAnswer } from './generated/WallClockAnswer';
import type { WallClockQuestion } from './generated/WallClockQuestion';
import type { WallReading } from './generated/WallReading';

export type { EndReading, SeriesEnd, WallClockAnswer, WallClockQuestion, WallReading };

/** This surface's door into the core: a question as JSON in, the answer as
 *  JSON out, JSON `null` for text that names no instant, date or time. */
export interface WallClockRules {
  wallClockJson(questionJson: string): string;
}

let installedRules: WallClockRules | null = null;

/** Bind this surface's door into the core. */
export function installWallClockRules(rules: WallClockRules): void {
  installedRules = rules;
}

/** One question, its answer; `null` where its text names no instant, date or
 *  time (a field being typed). A door that throws is at fault and stays loud. */
export function askWallClock(question: WallClockQuestion): WallClockAnswer | null {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'wall clock rules used before installWallClockRules() — the surface must ' +
        'bind its door into cal_core::wall_clock at startup',
    );
  }
  return JSON.parse(installedRules.wallClockJson(JSON.stringify(question))) as WallClockAnswer | null;
}

/** The date (`YYYY-MM-DD`) and time of day (`HH:MM`) the instant `at` shows on
 *  the clock of a series storing `tzid`. */
export function wallOf(at: string, tzid: string | null): WallClockAnswer | null {
  return askWallClock({ op: 'wallOf', at, tzid });
}

/** How `date` at `time` reads on the clock of a series storing `tzid`: once,
 *  twice (the earlier instant first), or not at all (a gap). */
export function instantOf(date: string, time: string, tzid: string | null): WallClockAnswer | null {
  return askWallClock({ op: 'instantOf', date, time, tzid });
}

/** The instant a repeat ending on `date` ends at: 23:59:59 that day on the
 *  clock of a series storing `tzid`; no `at` where the clock skips the day. */
export function untilOn(date: string, tzid: string | null): WallClockAnswer | null {
  return askWallClock({ op: 'untilOn', date, tzid });
}

/**
 * How a series' stored `UNTIL` reads on the clock it repeats on: through a
 * date, an instant, or a wall time, with the bound as a wall time, the last
 * instant at or before it and the last day it reaches
 * (`cal_core::read_series_end`, decisions 101b and 105a to 116a). `null` for a
 * start that names no instant; no `end` in the answer for a rule without an
 * `UNTIL`, with a `COUNT`, or one that cannot be read.
 *
 * `deviceZone` is the zone as this runtime reports it; only a series of days
 * reads it, on the device's days (decision 48a).
 */
export function seriesEnd(
  rrule: string,
  start: string,
  allDay: boolean,
  tzid: string | null,
  deviceZone: string | null,
): WallClockAnswer | null {
  return askWallClock({ op: 'seriesEnd', rrule, start, allDay, tzid, deviceZone });
}
