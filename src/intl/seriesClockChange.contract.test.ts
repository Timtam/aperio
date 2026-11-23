import { describe, expect, it } from 'vitest';
import { rrulestr } from 'rrule';

import contract from '../../crates/cal-core/tests/fixtures/seriesClockChange.json';
import { seriesClockChange, type ClockChangeQuestion } from '@aperio/shared';

/**
 * Changing the clock a series repeats on (stage 9 of
 * DESIGN-series-time-zone.md), pinned by
 * `crates/cal-core/tests/fixtures/seriesClockChange.json`: every row through
 * the door this surface installs, the real WebAssembly module. The core and
 * the phone's bridge read the same file.
 */
describe('seriesClockChange contract', () => {
  // Anti-silence: the rows the rule turns on, by name.
  it('still carries the rows the rule turns on', () => {
    const names = new Set(contract.rows.map((row) => row.name));
    for (const needed of [
      'lost-berlin-zone-monday-rule',
      'a-series-that-means-utc-sundays',
      'same-day-moves-cancelling-exceptions-only',
      'monthly-on-the-31st-cannot-keep-its-days',
      'moved-start-in-the-spring-gap',
      'a-cut-moves-as-the-end-of-its-last-day',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const row of contract.rows) {
    it(`answers ${row.name}`, () => {
      expect(seriesClockChange(row.question as ClockChangeQuestion), row.note).toEqual(row.expect);
    });
  }
});

/**
 * Stage 9's check before it was built: whether the core's reading of which
 * days a rule names agrees with the views' expansion (rrule.js) for counted
 * weekdays and days counted from the end of the month. Asked through the
 * door with the same zone on both sides, so the start's day is the day in
 * question and `fit` says whether the rule names it.
 */
describe('the days a rule names, against rrule.js', () => {
  const rules = [
    'FREQ=MONTHLY;BYDAY=2SU',
    'FREQ=MONTHLY;BYDAY=-1FR',
    'FREQ=MONTHLY;BYDAY=1MO,-1MO',
    'FREQ=MONTHLY;BYMONTHDAY=-1',
    'FREQ=MONTHLY;BYMONTHDAY=-2,15',
    'FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU',
    'FREQ=YEARLY;BYMONTH=10;BYDAY=2MO',
    'FREQ=YEARLY;BYDAY=1MO',
    'FREQ=YEARLY;BYDAY=-1SU',
    'FREQ=MONTHLY;BYDAY=TU;BYMONTHDAY=1,2,3,4,5,6,7',
  ];
  const from = Date.UTC(2026, 0, 1);
  const days = 2 * 366;

  for (const rule of rules) {
    it(`agrees for ${rule}`, () => {
      const named = new Set(
        rrulestr(`DTSTART:20260101T120000Z\nRRULE:${rule}`)
          .between(new Date(from), new Date(from + days * 86_400_000), true)
          .map((d) => d.toISOString().slice(0, 10)),
      );
      const disagree: string[] = [];
      for (let i = 0; i < days; i += 1) {
        const day = new Date(from + i * 86_400_000).toISOString().slice(0, 10);
        const answer = seriesClockChange({
          rrule: rule,
          start: `${day}T12:00:00Z`,
          exceptions: [],
          fromTzid: null,
          toTzid: null,
          endsByDate: false,
        });
        const fits = answer?.fit === 'both';
        if (answer?.fit === 'unknown' || fits !== named.has(day)) disagree.push(day);
      }
      expect(disagree).toEqual([]);
    });
  }
});
