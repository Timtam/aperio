import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';

import { expandAll } from './recurrence';

interface Ev {
  id: string;
  start: string;
  end: string;
  all_day?: boolean;
  recurrence: { rrule: string; exceptions: string[]; tzid?: string | null } | null;
  cancelled?: boolean;
}

interface Occurrence {
  event: number;
  start: string;
}

interface Row {
  name: string;
  note: string;
  wasTypeScript?: boolean;
  surfacesDiffer?: boolean;
  input: { events: Ev[]; range: { start: string; end: string } };
  expect: Occurrence[];
  reminders?: Occurrence[];
}

/**
 * Event recurrence, pinned as a table before it moves.
 *
 * `expect` is what the calendar views and the widget get from `expandAll`. The
 * reminder half of the same rows (host-core's `expand_occurrences`) is replayed
 * by `event_occurrence_contract` in `crates/host-core/src/reminders.rs`; a row
 * that records `reminders` is one where the two answer differently, and each
 * such row is a decision for the port.
 *
 * What stays out, and why, is written in the table's `notInThisTable`.
 */
const table = JSON.parse(
  readFileSync(resolve(process.cwd(), 'shared/contracts/eventOccurrences.json'), 'utf8'),
) as { cases: Row[] };

/** The views' answer as positions and instants. */
function answer(row: Row): Occurrence[] {
  const events = row.input.events;
  return expandAll(events, {
    start: new Date(row.input.range.start),
    end: new Date(row.input.range.end),
  }).map((o) => {
    const byRef = events.indexOf(o);
    const seriesId = (o as { series_id?: string }).series_id;
    return { event: byRef >= 0 ? byRef : events.findIndex((e) => e.id === seriesId), start: o.start };
  });
}

/**
 * Instants compare as instants, never as text, and in the table's order: by
 * instant, then input index. expandAll sorts its output by the start TEXT, so
 * its own order would make a plain event and an occurrence at the same
 * instant swap places depending on how each start is spelled.
 */
const instants = (rows: Occurrence[]) =>
  rows
    .map((r) => ({ event: r.event, at: Date.parse(r.start) }))
    .sort((a, b) => a.at - b.at || a.event - b.event);

describe('eventOccurrences contract (views)', () => {
  // An ALL-DAY series repeats on the device's calendar days (48a), so the
  // device's zone is an input to these rows — and the table names the one they
  // were measured on. It is answered here, rather than left to the machine, so
  // the rows mean the same on CI (which runs in UTC and would never cross a
  // clock change) as on a developer's laptop. Only the question "where is this
  // device" is answered; every zone the rows name is still read from the data.
  const machineZone = (table as { measuredWith?: { machineZone?: string } }).measuredWith
    ?.machineZone;
  // The restore, not the spy: naming a spy's type here would name the type
  // of what it wraps, and `resolvedOptions` answers a shape of its own.
  let restoreZone: (() => void) | null = null;
  beforeAll(() => {
    expect(machineZone, 'the table names the zone it was measured on').toBeTruthy();
    const real = Intl.DateTimeFormat.prototype.resolvedOptions;
    const spy = vi
      .spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions')
      .mockImplementation(function (this: Intl.DateTimeFormat) {
        const options = real.call(this);
        // Only a formatter built WITHOUT a zone asks "where am I"; one built
        // for a named zone keeps its own answer.
        return options.timeZone === real.call(new Intl.DateTimeFormat()).timeZone
          ? { ...options, timeZone: machineZone as string }
          : options;
      });
    restoreZone = () => spy.mockRestore();
  });
  afterAll(() => {
    restoreZone?.();
  });

  // Anti-silence: named rows, not a count.
  it('still carries the rows the rules turn on', () => {
    const names = new Set(table.cases.map((r) => r.name));
    for (const n of [
      'both-range-ends-are-inclusive',
      'an-occurrence-that-began-before-the-range',
      'a-date-only-until-without-a-zone',
      'a-date-only-until-on-a-zoned-series',
      'an-all-day-series-west-of-utc-until-its-local-day',
      'an-all-day-series-without-zone-into-winter',
      'an-all-day-series-into-summer-keeps-its-monday',
      'an-all-day-exception-cancels-the-day-it-names',
      'an-all-day-series-a-provider-gave-a-zone',
      'a-zoned-weekly-series-keeps-its-wall-clock-into-winter',
      'a-wall-clock-in-the-spring-gap',
      'a-wall-clock-in-the-autumn-overlap',
      'a-moved-occurrence-stands-in-for-its-slot',
      'a-cancelled-occurrence-removes-its-slot',
      'a-long-daily-series-in-a-wide-range',
      'a-date-only-until-on-a-zoned-series-at-one-in-the-morning',
      'an-until-without-z-before-the-wall-clock-time',
      'a-date-only-until-on-an-all-day-series-east-of-utc',
      'the-editors-until-on-an-all-day-series-east-of-utc',
      'a-zoned-until-with-z-across-the-autumn-change',
      'a-zoned-series-on-both-range-ends-into-summer',
      'a-zoned-occurrence-just-past-the-range-end',
      'a-moved-occurrence-of-a-zoned-series-after-the-change',
      'every-other-week-with-the-week-starting-on-sunday',
      'a-plain-event-at-the-same-instant-as-an-occurrence',
      'a-zone-with-surrounding-space',
      'a-lowercase-zone',
      'a-series-stored-as-etc-utc',
      'a-series-stored-as-gmt',
      'a-lowercase-utc-name',
      'an-offset-for-a-zone-name',
    ]) {
      expect(names, `table lost ${n}`).toContain(n);
    }
  });

  for (const row of table.cases) {
    it(row.name, () => {
      expect(instants(answer(row)), row.note).toEqual(instants(row.expect));
    });
  }
});
