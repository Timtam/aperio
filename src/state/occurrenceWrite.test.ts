import { describe, expect, it } from 'vitest';

import { occurrenceWrite, overrideIdFor } from '@aperio/shared';

/**
 * Decision 79b: "only this occurrence" writes a real exception inside the
 * series where the provider can hold one, and carves the occurrence out only
 * where it cannot. One rule, because six call sites ask it.
 */

const SERIES = 'cal/work.ics|uid-1';
const SLOT = '2026-06-15T07:00:00.000Z';

/** An occurrence as `expandAll` emits it: its own id, the master as series. */
const occurrence = () => ({
  id: `${SERIES}@${SLOT}`,
  series_id: SERIES,
  occurrence_start: SLOT,
  start: SLOT,
  end: '2026-06-15T08:00:00.000Z',
  recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [], tzid: null },
});

/** The exception the provider already holds. */
const override = () => ({
  id: `${SERIES}::rid::2026-06-15T07:00:00Z`,
  start: '2026-06-15T09:00:00.000Z',
  end: '2026-06-15T10:00:00.000Z',
  recurrence: null,
});

const holds = { stores_occurrence_exceptions: true };
const carves = { stores_occurrence_exceptions: false };

describe('occurrenceWrite', () => {
  it('writes the occurrence in place where the calendar holds one', () => {
    expect(
      occurrenceWrite({ row: occurrence(), calendar: holds, scope: 'occurrence' }),
    ).toEqual({
      kind: 'in-place',
      id: `${SERIES}::rid::2026-06-15T07:00:00Z`,
      occurrence: SLOT,
    });
  });

  it('carves it out where the calendar cannot, as every calendar did before', () => {
    expect(
      occurrenceWrite({ row: occurrence(), calendar: carves, scope: 'occurrence' }),
    ).toEqual({ kind: 'carve-out', seriesId: SERIES, occurrence: SLOT });
    // Nothing known about the calendar is not a promise either.
    expect(
      occurrenceWrite({ row: occurrence(), calendar: null, scope: 'occurrence' }),
    ).toEqual({ kind: 'carve-out', seriesId: SERIES, occurrence: SLOT });
    expect(
      occurrenceWrite({ row: occurrence(), calendar: {}, scope: 'occurrence' }),
    ).toEqual({ kind: 'carve-out', seriesId: SERIES, occurrence: SLOT });
  });

  it('writes an exception it already is through its own id, whatever the listing says', () => {
    // The row IS the exception. A stale capability must not turn a write that
    // works today into a carve-out.
    expect(
      occurrenceWrite({ row: override(), occurrence: SLOT, calendar: carves, scope: 'occurrence' }),
    ).toEqual({ kind: 'in-place', id: override().id, occurrence: SLOT });
  });

  it('takes the slot the phone carries beside its row', () => {
    // The phone's editor opens the SERIES MASTER and remembers the slot next
    // to it, so the row alone says nothing about which occurrence is meant.
    const master = {
      id: SERIES,
      start: '2026-06-01T07:00:00.000Z',
      end: '2026-06-01T08:00:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [], tzid: null },
    };
    expect(
      occurrenceWrite({ row: master, occurrence: SLOT, calendar: holds, scope: 'occurrence' }),
    ).toEqual({ kind: 'in-place', id: overrideIdFor(SERIES, SLOT), occurrence: SLOT });
    // Without a slot there is no occurrence to write, and guessing one would
    // write the wrong day.
    expect(occurrenceWrite({ row: master, calendar: holds, scope: 'occurrence' })).toEqual({
      kind: 'series',
    });
  });

  it('never answers for a scope that means the series', () => {
    for (const scope of ['series', 'this_and_future'] as const) {
      expect(occurrenceWrite({ row: occurrence(), calendar: holds, scope })).toEqual({
        kind: 'series',
      });
    }
  });

  it('carves out for another calendar, because the series is not going', () => {
    // A move or a copy to another calendar cannot be an exception: an
    // exception only exists inside its own series' resource.
    expect(
      occurrenceWrite({
        row: occurrence(),
        calendar: holds,
        scope: 'occurrence',
        destination: 'another-calendar',
      }),
    ).toEqual({ kind: 'carve-out', seriesId: SERIES, occurrence: SLOT });
    // Even an exception the provider already holds: moving it away ends it.
    expect(
      occurrenceWrite({
        row: override(),
        occurrence: SLOT,
        calendar: holds,
        scope: 'occurrence',
        destination: 'another-calendar',
      }),
    ).toEqual({ kind: 'carve-out', seriesId: SERIES, occurrence: SLOT });
  });
});

describe('overrideIdFor', () => {
  it('addresses the occurrence the way every adapter reads it', () => {
    // `{series}::rid::{slot}`, to the second — the shape
    // `cal_core::split_override_id` parses and the adapters mint.
    expect(overrideIdFor(SERIES, SLOT)).toBe(`${SERIES}::rid::2026-06-15T07:00:00Z`);
    // An unreadable slot is handed on as it came: the adapter refuses it by
    // name rather than this inventing an instant.
    expect(overrideIdFor(SERIES, 'not-a-date')).toBe(`${SERIES}::rid::not-a-date`);
  });
});
