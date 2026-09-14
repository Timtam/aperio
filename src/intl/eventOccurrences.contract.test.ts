import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

import { expandAll } from './recurrence';

interface Ev {
  id: string;
  start: string;
  end: string;
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
  // Anti-silence: named rows, not a count.
  it('still carries the rows the rules turn on', () => {
    const names = new Set(table.cases.map((r) => r.name));
    for (const n of [
      'both-range-ends-are-inclusive',
      'an-occurrence-that-began-before-the-range',
      'a-date-only-until-without-a-zone',
      'a-date-only-until-on-a-zoned-series',
      'an-all-day-series-west-of-utc-until-its-local-day',
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
