// Dates and times on the clock a series repeats on — this surface's door into
// `cal_core::wall_clock` (DESIGN-series-time-zone.md, stage 8, decision 100a).
//
// A series' start and end are typed and shown on its own clock: the zone its
// stored name names, or UTC. The core turns a date and a time of day into an
// instant and back with the tzdata the adapters write with: a time the clock
// skips (the spring gap) has no instant and says so, a time it shows twice
// (the autumn fold) takes the earlier reading, and a stored repeat end reads
// through a date or as an instant (`seriesEnd`). Every end Aperio writes is
// spelled here too: "through day D" (`untilThrough`) and "just before this
// occurrence" (`cutBefore`), so no end Aperio writes needs the 105a guard to
// be read right (DESIGN-series-time-zone.md, 142a). The desktop asks through
// its WebAssembly module, which carries the zones; the phone through its
// native core. Both answer synchronously, so a form can ask while it renders.
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

/**
 * The `UNTIL` that ends a series through `date` on its clock
 * (`cal_core::until_through`): `DT235959Z` where that reads through the date
 * on these facts, else the date's last instant on the series' clock, as `at`.
 * No reader needs the 105a guard to read it. `null` for text that names no
 * date or instant.
 */
export function untilThrough(
  date: string,
  rrule: string,
  start: string,
  allDay: boolean,
  tzid: string | null,
): WallClockAnswer | null {
  return askWallClock({ op: 'untilThrough', date, rrule, start, allDay, tzid });
}

/**
 * The `UNTIL` that ends a series just before the occurrence at `at`
 * (`cal_core::cut_before`, 106a): one second before it, two where one would
 * spell `…T235959Z`, except in a rule that repeats by the second. `null` for
 * text that names no instant.
 */
export function cutBefore(at: string, rrule: string): WallClockAnswer | null {
  return askWallClock({ op: 'cutBefore', at, rrule });
}
