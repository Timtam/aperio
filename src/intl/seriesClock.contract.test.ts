import { describe, expect, it, vi } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/seriesClock.json';
import slotContract from '../../crates/cal-core/tests/fixtures/movedSlots.json';
import { canonicalZone, movedSlots, seriesClockZone, type SlotMoveQuestion } from '@aperio/shared';

interface ClockRow {
  tzid: string | null;
  zone: string | null;
  note: string;
}

interface CanonicalRow {
  name: string;
  canonical: string | null;
  note: string;
}

const clockRows = contract.seriesClockZone as ClockRow[];
const canonicalRows = contract.canonicalZone as CanonicalRow[];

/**
 * Which stored zone names a series repeats on, through the real WebAssembly
 * door and the shared shell — the path every view takes. The core reads the
 * same rows in `crates/cal-core/src/series_clock.rs`, and the phone's doors in
 * `crates/cal-ffi/src/lib.rs`.
 */
describe('seriesClock contract (WebAssembly door and shared shell)', () => {
  // Anti-silence: named rows, not a count.
  it('still carries the rows the rule turns on', () => {
    const tzids = new Set(clockRows.map((r) => r.tzid));
    for (const tzid of [
      null,
      'etc/utc',
      'GMT+0',
      ' UTC',
      'Etc/Unıversal',
      'Asia/Calcutta',
      'Europe/Oslo',
      'Etc/GMT+8',
      '+05:30',
      'europe/berlin',
    ]) {
      expect(tzids, `table lost ${JSON.stringify(tzid)}`).toContain(tzid);
    }
    const names = new Set(canonicalRows.map((r) => r.name));
    for (const name of ['asia/calcutta', 'Europe/Oslo', 'Etc/GMT+8', 'UTC', 'Etc/Unıversal', '']) {
      expect(names, `table lost ${JSON.stringify(name)}`).toContain(name);
    }
  });

  for (const row of clockRows) {
    it(`seriesClockZone(${JSON.stringify(row.tzid)})`, () => {
      expect(seriesClockZone(row.tzid), row.note).toBe(row.zone);
    });
  }

  for (const row of canonicalRows) {
    it(`canonicalZone(${JSON.stringify(row.name)})`, () => {
      expect(canonicalZone(row.name), row.note).toBe(row.canonical);
    });
  }

  it('reads an absent zone as no zone', () => {
    expect(seriesClockZone(undefined)).toBeNull();
  });

});

/**
 * Where a series' changed occurrences stand on its new clock (stage 13, 95a),
 * pinned by `crates/cal-core/tests/fixtures/movedSlots.json`: every row through
 * the real WebAssembly door. The core and the phone's door read the same file;
 * `seriesClockChange.contract.test.ts` replays the clock-change answers, with
 * the `movedMinutes` these slots move by.
 */
describe('movedSlots contract (WebAssembly door and shared shell)', () => {
  // Anti-silence: the rows the rule turns on, by name.
  it('still carries the rows the rule turns on', () => {
    const names = new Set(slotContract.rows.map((row) => row.name));
    for (const needed of [
      'as-now-in-the-other-half-year',
      'lost-berlin-zone-monday-rule-as-the-rule-says',
      'every-other-week',
      'a-slot-that-names-no-occurrence',
      'a-slot-after-the-end',
      'a-slot-moved-into-the-spring-gap',
      'a-start-that-names-no-instant',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const row of slotContract.rows) {
    it(`moves ${row.name}`, () => {
      expect(movedSlots(row.question as SlotMoveQuestion), row.note).toEqual(row.expect);
    });
  }
});

describe('the shell around the door', () => {
  // A module graph of its own, with a door that answers every zone in one
  // spelling of its own: the shell must still hand on the name the surface
  // stored, never the string the door answered with.
  it('hands on the stored name, not the door answer', async () => {
    vi.resetModules();
    const shell = await import('../../shared/seriesClock');
    shell.installSeriesClockRules({
      seriesClockZone: (tzid) => (tzid === '' ? '' : 'Europe/Berlin'),
      canonicalZone: () => '',
      expansionClock: () => 'zone',
      seriesClockChangeJson: () => 'null',
      movedSlotsJson: () => 'null',
    });
    expect(shell.seriesClockZone('europe/berlin')).toBe('europe/berlin');
    expect(shell.seriesClockZone(null)).toBeNull();
  });
});

describe('seriesClock before its door is installed', () => {
  // A module graph of its own, in which nothing has installed the door: the
  // state a surface is in if it forgets to. The rule must refuse loudly there,
  // and `localTimeZone` must not swallow that as "the device has no zone".
  it('throws from the shell and from localTimeZone', async () => {
    vi.resetModules();
    const shell = await import('../../shared/seriesClock');
    const recurrence = await import('../../shared/recurrence');
    expect(() => shell.seriesClockZone('Europe/Berlin')).toThrow(/installSeriesClockRules/);
    expect(() => recurrence.localTimeZone()).toThrow(/installSeriesClockRules/);
    // Nor may the repeat sentence read it as a rule without a last day.
    expect(() =>
      recurrence.lastOccurrenceDayKey({
        id: 'e',
        start: '2026-06-15T07:00:00.000Z',
        end: '2026-06-15T08:00:00.000Z',
        recurrence: { rrule: 'FREQ=WEEKLY;UNTIL=20261231T235959Z', exceptions: [], tzid: 'Europe/Berlin' },
      }),
    ).toThrow(/installSeriesClockRules/);
  });
});
