import { describe, expect, it, vi } from 'vitest';

import {
  canonicalZoneThroughCore,
  expansionClockThroughCore,
  seriesClockZoneThroughCore,
} from '../wasm/coreRules';

import {
  firstOccurrenceFrom,
  futureCarryRow,
  occurrenceOfSeries,
  planSeriesSplit,
  readSeriesRows,
  ruleFromCut,
  seriesFromCut,
  seriesMaybeShownTwice,
  cutoffDay,
  deletedSlots,
  expandAll,
  installSeriesClockRules,
  seriesRowsFromHost,
  thisAndFutureDeletedKey,
  truncateRRuleBefore,
  writeSeriesSplit,
  type SeriesCutPlan,
  type SeriesSplitPlan,
  type WholeSeriesPlan,
} from '@aperio/shared';

/** A weekly Monday series, ten times, starting 2026-08-03. */
const weekly = {
  id: 'ev-1',
  start: '2026-08-03T08:00:00.000Z',
  end: '2026-08-03T09:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;COUNT=10',
    exceptions: [] as string[],
    tzid: null as string | null,
  },
};

/** The plan, which must keep a head. */
function cutOf(plan: SeriesSplitPlan | null): SeriesCutPlan {
  expect(plan?.kind).toBe('cut');
  return plan as SeriesCutPlan;
}

/** The plan, which must have no head. */
function wholeOf(plan: SeriesSplitPlan | null): WholeSeriesPlan {
  expect(plan?.kind).toBe('whole');
  return plan as WholeSeriesPlan;
}

describe('planSeriesSplit', () => {
  it('ends the head before the cutoff and gives the tail what is left', () => {
    // Split at the fourth occurrence: three stay, seven move.
    const plan = planSeriesSplit(weekly, '2026-08-24T08:00:00.000Z', []);
    expect(plan).not.toBeNull();
    expect(plan?.occurrencesBefore).toBe(3);
    expect(cutOf(plan).headRule).toContain('UNTIL=20260824T075959Z');
    expect(cutOf(plan).headRule).not.toContain('COUNT');
    expect(plan?.tail.rrule).toBe('FREQ=WEEKLY;COUNT=7');
  });

  it('counts the slots an EXDATE suppressed, not the visible occurrences', () => {
    // The second Monday was deleted. RFC-5545 COUNT counts it anyway, so the
    // tail must still be seven — counting what is VISIBLE would make it eight
    // and append an appointment past the end of the series.
    const withHole = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-08-10T08:00:00.000Z'] },
    };
    const plan = planSeriesSplit(withHole, '2026-08-24T08:00:00.000Z', []);
    expect(plan?.occurrencesBefore).toBe(3);
    expect(plan?.tail.rrule).toBe('FREQ=WEEKLY;COUNT=7');
  });

  it('hands the tail the exceptions that belong to it, and keeps the rest out', () => {
    // One deleted occurrence on each side of the cutoff. The later one has to
    // travel: left behind, the tail would resurrect an appointment the user
    // explicitly deleted.
    const withHoles = {
      ...weekly,
      recurrence: {
        ...weekly.recurrence,
        exceptions: ['2026-08-10T08:00:00.000Z', '2026-09-07T08:00:00.000Z'],
      },
    };
    const plan = planSeriesSplit(withHoles, '2026-08-24T08:00:00.000Z', []);
    expect(plan?.tail.exceptions).toEqual(['2026-09-07T08:00:00.000Z']);
  });

  it('carries the zone verbatim, floating included', () => {
    const zoned = {
      ...weekly,
      recurrence: { ...weekly.recurrence, tzid: 'Europe/Berlin' },
    };
    expect(planSeriesSplit(zoned, '2026-08-24T08:00:00.000Z', [])?.tail.tzid).toBe(
      'Europe/Berlin',
    );
    // A floating series stays floating: stamping only the tail would make the
    // two halves expand an hour apart across a DST boundary.
    expect(planSeriesSplit(weekly, '2026-08-24T08:00:00.000Z', [])?.tail.tzid).toBeNull();
  });

  it('leaves an open-ended series open', () => {
    const open = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY' },
    };
    const plan = planSeriesSplit(open, '2026-08-24T08:00:00.000Z', []);
    expect(plan?.tail.rrule).toBe('FREQ=WEEKLY');
    expect(cutOf(plan).headRule).toContain('UNTIL=');
  });

  it('emits a date-only UNTIL for an all-day series, on the day BEFORE the cut', () => {
    // A DATE-valued series needs a DATE-valued UNTIL, or strict providers drop
    // the rule outright — and UNTIL is INCLUSIVE in date space, so it must name
    // the day before the split or the occurrence the user split away stays in
    // the truncated half as well, existing twice.
    //
    // The instants are built as LOCAL midnight, which is what an all-day event
    // actually holds (adapter-caldav's `naive_date_to_utc`). Read as UTC days
    // the bound lands on the cutoff day itself west of Greenwich, which is the
    // whole point of the local reading in `formatRRuleUntilDate`.
    const localMidnight = (y: number, m: number, d: number) =>
      new Date(y, m - 1, d).toISOString();
    const allDay = {
      ...weekly,
      all_day: true,
      start: localMidnight(2026, 8, 3),
      end: localMidnight(2026, 8, 4),
    };
    const plan = planSeriesSplit(allDay, localMidnight(2026, 8, 24), []);
    expect(cutOf(plan).headRule).toContain('UNTIL=20260823');
    expect(cutOf(plan).headRule).not.toContain('UNTIL=20260823T');
  });

  it('keeps a timed series UNTIL to the second', () => {
    // The counterpart: a timed series' UNTIL is a UTC datetime, one second
    // before the cutoff, and must NOT be read in local terms.
    const plan = planSeriesSplit(weekly, '2026-08-24T08:00:00.000Z', []);
    expect(cutOf(plan).headRule).toContain('UNTIL=20260824T075959Z');
  });

  it('truncates an all-day series to the day before, in local terms', () => {
    // Straight at the rule helper, since this is where the day is decided.
    const cut = new Date(2026, 7, 24);
    const rule = truncateRRuleBefore('FREQ=WEEKLY;COUNT=10', cut, { allDay: true });
    const p = (n: number) => String(n).padStart(2, '0');
    const dayBefore = new Date(cut.getTime() - 86_400_000);
    expect(rule).toContain(
      `UNTIL=${dayBefore.getFullYear()}${p(dayBefore.getMonth() + 1)}${p(
        dayBefore.getDate(),
      )}`,
    );
  });

  // Decision 118: nothing the calendar shows before the cutoff means no head.
  // Cutting anyway wrote a rule that ends before it starts — hidden from the
  // views, and the reminders fell back to the series start and rang on.
  it('has no head at the first occurrence', () => {
    const plan = wholeOf(planSeriesSplit(weekly, weekly.start, []));
    expect(plan.occurrencesBefore).toBe(0);
    expect(plan.tail.rrule).toBe('FREQ=WEEKLY;COUNT=10');
    expect('headRule' in plan).toBe(false);
  });

  it('has no head when the series starts off its own pattern', () => {
    // Monday start, Wednesdays only: the rule's first occurrence is the
    // Wednesday, and nothing comes before it.
    const offPattern = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY;BYDAY=WE;COUNT=10' },
    };
    const plan = wholeOf(planSeriesSplit(offPattern, '2026-08-05T08:00:00.000Z', []));
    expect(plan.occurrencesBefore).toBe(0);
  });

  it('has no head when every earlier occurrence was deleted', () => {
    // The first two Mondays are gone; cutting at the third leaves nothing
    // shown before it — though COUNT still counts them, so the series keeps
    // eight from here.
    const bothGone = {
      ...weekly,
      recurrence: {
        ...weekly.recurrence,
        exceptions: ['2026-08-03T08:00:00.000Z', '2026-08-10T08:00:00.000Z'],
      },
    };
    const plan = wholeOf(planSeriesSplit(bothGone, '2026-08-17T08:00:00.000Z', []));
    expect(plan.occurrencesBefore).toBe(2);
    expect(plan.tail.rrule).toBe('FREQ=WEEKLY;COUNT=8');
    expect(plan.tail.exceptions).toEqual([]);
  });

  it('keeps a head when one earlier occurrence is still shown', () => {
    const oneGone = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-08-03T08:00:00.000Z'] },
    };
    cutOf(planSeriesSplit(oneGone, '2026-08-17T08:00:00.000Z', []));
  });

  it('has no head at the first day of an all-day series, or of a zoned one', () => {
    const localMidnight = (y: number, m: number, d: number) =>
      new Date(y, m - 1, d).toISOString();
    const allDay = {
      ...weekly,
      all_day: true,
      start: localMidnight(2026, 8, 3),
      end: localMidnight(2026, 8, 4),
    };
    wholeOf(planSeriesSplit(allDay, allDay.start, []));
    const zoned = {
      ...weekly,
      recurrence: { ...weekly.recurrence, tzid: 'Europe/Berlin' },
    };
    wholeOf(planSeriesSplit(zoned, zoned.start, []));
  });

  // Decision 125: the master's exceptions alone cannot say what is shown.
  // Exchange lists the slot of an occurrence changed in Outlook among them,
  // and shows it as a row of its own.
  it('keeps a head when an earlier occurrence was changed, not deleted (Exchange)', () => {
    const changedFirst = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-08-03T08:00:00.000Z'] },
    };
    const moved = {
      ...weekly,
      id: 'ev-1::rid::2026-08-03T08:00:00.000Z',
      start: '2026-08-04T12:00:00.000Z',
      end: '2026-08-04T13:00:00.000Z',
      recurrence: null,
    };
    cutOf(planSeriesSplit(changedFirst, '2026-08-10T08:00:00.000Z', [moved]));
    // Without its row, the same master reads as nothing before.
    wholeOf(planSeriesSplit(changedFirst, '2026-08-10T08:00:00.000Z', []));
  });

  it('has no head when the only earlier occurrence was cancelled (Google)', () => {
    // Google writes no exception: it keeps the deleted occurrence as a
    // cancelled row of its own.
    const cancelled = {
      ...weekly,
      id: 'ev-1::rid::2026-08-03T08:00:00.000Z',
      recurrence: null,
      cancelled: true,
    };
    wholeOf(planSeriesSplit(weekly, '2026-08-10T08:00:00.000Z', [cancelled]));
  });

  it("counts a row by its slot, and ignores another series' rows", () => {
    const firstGone = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-08-03T08:00:00.000Z'] },
    };
    // Another series' row on the same day is not this series' head.
    const other = {
      ...weekly,
      id: 'ev-9::rid::2026-08-03T08:00:00.000Z',
      recurrence: null,
    };
    wholeOf(planSeriesSplit(firstGone, '2026-08-10T08:00:00.000Z', [other]));
    // This series' row for a LATER slot, moved before the cutoff, is not in
    // the head either: a cut drops it.
    const laterMovedEarly = {
      ...weekly,
      id: 'ev-1::rid::2026-08-17T08:00:00.000Z',
      start: '2026-08-05T08:00:00.000Z',
      end: '2026-08-05T09:00:00.000Z',
      recurrence: null,
    };
    wholeOf(planSeriesSplit(firstGone, '2026-08-10T08:00:00.000Z', [laterMovedEarly]));
  });

  it("does not hide a new series' first occurrence behind a changed slot's exception", () => {
    // Exchange: the occurrence cut at was changed in Outlook, so its slot is
    // an exception of the master. The new series starts on that slot.
    const changedAtCut = {
      ...weekly,
      recurrence: {
        ...weekly.recurrence,
        exceptions: ['2026-08-24T08:00:00.000Z', '2026-09-07T08:00:00.000Z'],
      },
    };
    const plan = cutOf(planSeriesSplit(changedAtCut, '2026-08-24T08:00:00.000Z', []));
    expect(plan.tail.exceptions).toEqual(['2026-09-07T08:00:00.000Z']);
  });

  it("keeps the exception on the slot when the series is written whole", () => {
    // In place, the provider's row for that slot goes on standing in for it.
    const changedFirst = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-08-03T08:00:00.000Z'] },
    };
    const plan = wholeOf(planSeriesSplit(changedFirst, weekly.start, []));
    expect(plan.tail.exceptions).toEqual(['2026-08-03T08:00:00.000Z']);
  });

  it('refuses an event that carries no rule', () => {
    // The caller must not fall through to a whole-series edit on this: that
    // moves every occurrence, which is what the scope question prevents.
    const single = { ...weekly, recurrence: null };
    expect(planSeriesSplit(single, '2026-08-24T08:00:00.000Z', [])).toBeNull();
  });
});

/** A row the provider keeps for one occurrence of `weekly` (`ev-1`), shown
 *  where `start` says, standing in for `slot`. */
type RecurringRow = {
  id: string;
  start: string;
  end: string;
  all_day: boolean;
  cancelled?: boolean;
  recurrence: null;
};
const rowOf = (
  slot: string,
  shown: { start: string; end: string } = {
    start: slot,
    end: new Date(Date.parse(slot) + 3_600_000).toISOString(),
  },
  extra: Partial<RecurringRow> = {},
  series = 'ev-1',
): RecurringRow => ({
  id: `${series}::rid::${slot}`,
  start: shown.start,
  end: shown.end,
  all_day: false,
  recurrence: null,
  ...extra,
});
/** The weekly series, a Monday at 08:00 UTC, without an end. */
const endless = {
  ...weekly,
  recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY' },
};
const CUT = '2026-08-24T08:00:00.000Z';

describe('deletedSlots (decision 134)', () => {
  it('reads a cancelled row as a deleted occurrence, with no exception for it', () => {
    // Google keeps a deleted occurrence that way.
    const cancelled = rowOf('2026-09-07T08:00:00Z', undefined, { cancelled: true, start: '2026-09-07T08:00:00Z', end: '2026-09-07T08:00:00Z' });
    expect(deletedSlots(weekly, [cancelled])).toEqual(['2026-09-07T08:00:00.000Z']);
  });

  it('reads an exception with a live row as a changed occurrence, not a deleted one', () => {
    // Exchange lists the slot of an occurrence changed in Outlook among the
    // exceptions, and keeps the occurrence as a row of its own.
    const master = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-09-07T08:00:00.000Z'] },
    };
    const moved = rowOf('2026-09-07T08:00:00Z', {
      start: '2026-09-08T10:00:00.000Z',
      end: '2026-09-08T11:00:00.000Z',
    });
    expect(deletedSlots(master, [moved])).toEqual([]);
    // Without the row, the exception is a deletion, in its own spelling.
    expect(deletedSlots(master, [])).toEqual(['2026-09-07T08:00:00.000Z']);
  });

  it('names each slot once, in order, however it is spelled', () => {
    const master = {
      ...weekly,
      recurrence: {
        ...weekly.recurrence,
        exceptions: ['2026-09-14T08:00:00Z', '2026-09-07T08:00:00.000Z'],
      },
    };
    const cancelled = rowOf('2026-09-07T08:00:00Z', undefined, { cancelled: true });
    expect(deletedSlots(master, [cancelled])).toEqual([
      '2026-09-07T08:00:00.000Z',
      '2026-09-14T08:00:00Z',
    ]);
  });

  it('leaves the rows of other series alone', () => {
    const other = rowOf('2026-09-07T08:00:00Z', undefined, { cancelled: true }, 'ev-10');
    expect(deletedSlots(weekly, [other])).toEqual([]);
  });

  it('matches a day of a series of days however the day was spelled', () => {
    // A series of days names a day; another writer may have stored it hours
    // off the local midnight the occurrence has (decision 95).
    const monday = new Date(2026, 7, 31).toISOString();
    const days = {
      ...weekly,
      all_day: true,
      start: new Date(2026, 7, 3).toISOString(),
      end: new Date(2026, 7, 4).toISOString(),
      recurrence: { ...weekly.recurrence, exceptions: [monday] },
    };
    const sameDayLater = new Date(Date.parse(monday) + 5 * 3_600_000).toISOString();
    const changed = rowOf(sameDayLater, undefined, { all_day: true });
    // The exception's day has a live row: changed, not deleted.
    expect(deletedSlots(days, [changed])).toEqual([]);
  });
});

describe('planSeriesSplit: what the new series leaves out', () => {
  it('leaves out an occurrence Google keeps deleted as a cancelled row', () => {
    const cancelled = rowOf('2026-08-31T08:00:00Z', undefined, { cancelled: true });
    expect(cutOf(planSeriesSplit(weekly, CUT, [cancelled])).tail.exceptions).toEqual([
      '2026-08-31T08:00:00.000Z',
    ]);
  });

  it('leaves it out years ahead too', () => {
    // How far the rows reach is the host's to say; the rule takes them all.
    const later = rowOf('2028-08-28T08:00:00Z', undefined, { cancelled: true });
    expect(cutOf(planSeriesSplit(endless, CUT, [later])).tail.exceptions).toEqual([
      '2028-08-28T08:00:00.000Z',
    ]);
  });

  it('keeps an occurrence changed in Outlook, and deletes it from neither half', () => {
    const master = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-09-07T08:00:00.000Z'] },
    };
    const moved = rowOf('2026-09-07T08:00:00Z', {
      start: '2026-09-07T12:00:00.000Z',
      end: '2026-09-07T13:00:00.000Z',
    });
    expect(cutOf(planSeriesSplit(master, CUT, [moved])).tail.exceptions).toEqual([]);
  });

  it('leaves out a slot once when an exception and a cancelled row both name it', () => {
    const master = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: ['2026-09-07T08:00:00.000Z'] },
    };
    const cancelled = rowOf('2026-09-07T08:00:00Z', undefined, { cancelled: true });
    expect(cutOf(planSeriesSplit(master, CUT, [cancelled])).tail.exceptions).toEqual([
      '2026-09-07T08:00:00.000Z',
    ]);
  });

  it('still leaves out a plain exception, and nothing before the cut', () => {
    const master = {
      ...weekly,
      recurrence: {
        ...weekly.recurrence,
        exceptions: ['2026-08-10T08:00:00.000Z', '2026-09-07T08:00:00.000Z'],
      },
    };
    expect(cutOf(planSeriesSplit(master, CUT, [])).tail.exceptions).toEqual([
      '2026-09-07T08:00:00.000Z',
    ]);
  });
});

describe('firstOccurrenceFrom with the series\' rows (decision 141)', () => {
  it('finds an occurrence changed in Outlook on the cut day, by its slot', () => {
    // Exchange lists its slot among the exceptions; it is there all the same.
    // Read from the exceptions alone, the copy was cut a week late and kept
    // its old content on the cut day.
    const copy = {
      ...weekly,
      recurrence: { ...weekly.recurrence, exceptions: [CUT] },
    };
    const moved = rowOf('2026-08-24T08:00:00Z', {
      start: '2026-08-24T10:00:00.000Z',
      end: '2026-08-24T11:00:00.000Z',
    });
    expect(firstOccurrenceFrom(copy, CUT, [moved])).toBe(CUT);
  });

  it('passes over an occurrence Google keeps deleted as a cancelled row', () => {
    const cancelled = rowOf('2026-08-24T08:00:00Z', undefined, { cancelled: true });
    expect(firstOccurrenceFrom(weekly, CUT, [cancelled])).toBe('2026-08-31T08:00:00.000Z');
  });

  it('leaves the rows of other series alone', () => {
    const other = rowOf('2026-08-24T08:00:00Z', undefined, { cancelled: true }, 'ev-10');
    expect(firstOccurrenceFrom(weekly, CUT, [other])).toBe(CUT);
  });
});

describe('firstOccurrenceFrom', () => {
  it('is the cutoff itself when the series has an occurrence there', () => {
    expect(firstOccurrenceFrom(weekly, '2026-08-24T08:00:00.000Z', [])).toBe(
      '2026-08-24T08:00:00.000Z',
    );
  });

  it('is the occurrence already RUNNING at the cutoff, not the next one', () => {
    // Copies of one appointment often start a little apart — the work copy at
    // 09:00, the private one at 08:45 for the walk over. Selected by start
    // alone, the copy already under way at the cutoff counted as past and was
    // cut a whole period late: today stayed as it was and next week carried
    // the change.
    const startsEarlier = {
      ...weekly,
      start: '2026-08-03T07:45:00.000Z',
      end: '2026-08-03T09:00:00.000Z',
    };
    expect(firstOccurrenceFrom(startsEarlier, '2026-08-24T08:00:00.000Z', [])).toBe(
      '2026-08-24T07:45:00.000Z',
    );
  });

  it('is the copy own next occurrence when the patterns differ', () => {
    // The other copy of the same appointment runs FORTNIGHTLY, so it has
    // nothing on the cutoff day — its "and all following" starts a week later.
    const fortnightly = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY;INTERVAL=2;COUNT=10' },
    };
    expect(firstOccurrenceFrom(fortnightly, '2026-08-24T08:00:00.000Z', [])).toBe(
      '2026-08-31T08:00:00.000Z',
    );
  });

  it('finds an occurrence years out', () => {
    // A three-yearly series has nothing within any horizon short enough to keep
    // the common case cheap — and "nothing" is not harmless here: it tells the
    // carry this copy has no appointment left, and the copy is reported as one
    // it could not carry to.
    const everyThreeYears = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=YEARLY;INTERVAL=3;COUNT=5' },
    };
    expect(firstOccurrenceFrom(everyThreeYears, '2026-08-24T08:00:00.000Z', [])).toBe(
      '2029-08-03T08:00:00.000Z',
    );
  });

  it('is nothing when the series has already ended', () => {
    const short = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY;COUNT=2' },
    };
    expect(firstOccurrenceFrom(short, '2026-08-24T08:00:00.000Z', [])).toBeNull();
  });

  it('treats a single event as its own only occurrence', () => {
    const single = { ...weekly, recurrence: null };
    expect(firstOccurrenceFrom(single, '2026-08-03T08:00:00.000Z', [])).toBe(
      '2026-08-03T08:00:00.000Z',
    );
    expect(firstOccurrenceFrom(single, '2026-08-24T08:00:00.000Z', [])).toBeNull();
  });
});

describe('seriesFromCut', () => {
  it('is the master itself at the first occurrence', () => {
    const plan = wholeOf(planSeriesSplit(weekly, weekly.start, []));
    const series = seriesFromCut(weekly, plan, weekly.start);
    expect(series.start).toBe(weekly.start);
    expect(series.end).toBe(weekly.end);
    expect(series.recurrence).toEqual(weekly.recurrence);
  });

  it('starts where the series is first seen, with the COUNT that is left', () => {
    const bothGone = {
      ...weekly,
      recurrence: {
        ...weekly.recurrence,
        exceptions: ['2026-08-03T08:00:00.000Z', '2026-08-10T08:00:00.000Z'],
      },
    };
    const cut = '2026-08-17T08:00:00.000Z';
    const series = seriesFromCut(bothGone, wholeOf(planSeriesSplit(bothGone, cut, [])), cut);
    expect(series.start).toBe(cut);
    expect(series.end).toBe('2026-08-17T09:00:00.000Z');
    expect(series.recurrence).toEqual({
      rrule: 'FREQ=WEEKLY;COUNT=8',
      exceptions: [],
      tzid: null,
    });
  });
});

describe('ruleFromCut', () => {
  it('leaves what is left of a COUNT counted from the first occurrence', () => {
    expect(ruleFromCut('FREQ=WEEKLY;INTERVAL=2;COUNT=10', 4)).toBe(
      'FREQ=WEEKLY;INTERVAL=2;COUNT=6',
    );
    expect(ruleFromCut('RRULE:FREQ=DAILY;COUNT=3', 5)).toBe('FREQ=DAILY;COUNT=1');
    expect(ruleFromCut('FREQ=WEEKLY;UNTIL=20261231T235959Z', 4)).toBe(
      'FREQ=WEEKLY;UNTIL=20261231T235959Z',
    );
  });
});

describe('occurrenceOfSeries', () => {
  it('is the occurrence at the slot, shaped as the views expand it', () => {
    const occ = occurrenceOfSeries(weekly, '2026-08-24T08:00:00.000Z');
    expect(occ).toMatchObject({
      id: 'ev-1@2026-08-24T08:00:00.000Z',
      series_id: 'ev-1',
      occurrence_start: '2026-08-24T08:00:00.000Z',
      start: '2026-08-24T08:00:00.000Z',
      end: '2026-08-24T09:00:00.000Z',
      recurrence: weekly.recurrence,
    });
  });
});

describe('occurrenceOfSeries on a series of days', () => {
  it('opens on the day the slot names, however it was spelled', () => {
    // Decision 95: another writer may spell the day hours off this device's
    // local midnight. The occurrence opens on the series' own day, as the
    // views show it — not a day early west of Greenwich.
    const localMidnight = (y: number, m: number, d: number) =>
      new Date(y, m - 1, d).toISOString();
    const days = {
      ...weekly,
      all_day: true,
      start: localMidnight(2026, 8, 3),
      end: localMidnight(2026, 8, 4),
    };
    const day = new Date(2026, 7, 24).getTime();
    for (const offsetHours of [-5, 0, 5]) {
      const spelled = new Date(day + offsetHours * 3_600_000).toISOString();
      expect(occurrenceOfSeries(days, spelled).start).toBe(localMidnight(2026, 8, 24));
    }
  });
});

describe('readSeriesRows', () => {
  const own = { ...weekly, id: 'ev-1::rid::2026-08-03T08:00:00.000Z', recurrence: null };
  const other = { ...weekly, id: 'ev-9::rid::2026-08-03T08:00:00.000Z', recurrence: null };

  it("asks for the series by its id, and passes on how far the rows reach", async () => {
    const asked: unknown[] = [];
    const reach = {
      kind: 'window' as const,
      start: '2026-06-01T00:00:00Z',
      end: '2027-09-01T00:00:00Z',
    };
    const answer = await readSeriesRows(
      { ...weekly, calendar_id: 'cal' },
      '2026-08-24T08:00:00.000Z',
      async (request) => {
        asked.push(request);
        return { rows: [own], reach };
      },
    );
    expect(answer).toEqual({ rows: [own], reach });
    // The stretch is for a phone whose native library predates the series
    // read: a month around the series' start and the cutoff, as before.
    expect(asked).toEqual([
      {
        calendar_id: 'cal',
        series_id: 'ev-1',
        start: '2026-07-03T08:00:00.000Z',
        end: '2026-09-24T08:00:00.000Z',
      },
    ]);
  });

  it("keeps only the series' own rows from an older host's events", async () => {
    const answer = await readSeriesRows(
      { ...weekly, calendar_id: 'cal' },
      '2026-08-24T08:00:00.000Z',
      async () => seriesRowsFromHost([weekly, own, other]),
    );
    expect(answer).toEqual({ rows: [own], reach: { kind: 'unknown' } });
  });

  it('reads nothing for an event without a rule', async () => {
    const read = vi.fn();
    const answer = await readSeriesRows(
      { ...weekly, recurrence: null, calendar_id: 'cal' },
      '2026-08-24T08:00:00.000Z',
      read,
    );
    expect(read).not.toHaveBeenCalled();
    expect(answer).toEqual({ rows: [], reach: { kind: 'complete' } });
  });
});

describe('seriesRowsFromHost', () => {
  it("takes a current host's answer as it is", () => {
    const answer = { rows: [weekly], reach: { kind: 'complete' as const } };
    expect(seriesRowsFromHost(answer)).toBe(answer);
  });

  it("reads an older host's list of events as rows whose reach nobody knows", () => {
    expect(seriesRowsFromHost([weekly])).toEqual({
      rows: [weekly],
      reach: { kind: 'unknown' },
    });
  });
});

describe('thisAndFutureDeletedKey', () => {
  it('says the whole series went when nothing came before', () => {
    expect(thisAndFutureDeletedKey('deleted', false)).toBe(
      'dialogs.event.thisAndFutureDeletedWhole',
    );
    expect(thisAndFutureDeletedKey('deleted', true)).toBe(
      'dialogs.event.thisAndFutureCancelledWhole',
    );
    expect(thisAndFutureDeletedKey('truncated', false)).toBe(
      'dialogs.event.thisAndFutureDeleted',
    );
    expect(thisAndFutureDeletedKey('truncated', true)).toBe(
      'dialogs.event.thisAndFutureCancelled',
    );
  });
});

describe('writeSeriesSplit', () => {
  /** A host error, as both surfaces hand it over. */
  const coded = (code: string, message: string) => ({ code, message });

  it('refuses a series with nothing before the cutoff, and writes nothing', async () => {
    // The type refuses it already; this is the plan that got past it.
    const io = {
      createTail: vi.fn(async () => ({ id: 'tail' })),
      truncate: vi.fn(async () => undefined),
      removeTail: vi.fn(async () => undefined),
    };
    const whole = planSeriesSplit(weekly, weekly.start, []);
    await expect(writeSeriesSplit(io, whole as SeriesCutPlan)).rejects.toThrow();
    expect(io.createTail).not.toHaveBeenCalled();
    expect(io.truncate).not.toHaveBeenCalled();
  });

  const plan: SeriesCutPlan = {
    kind: 'cut',
    headRule: 'FREQ=WEEKLY;UNTIL=20260824T075959Z',
    tail: { rrule: 'FREQ=WEEKLY;COUNT=7', exceptions: [], tzid: null },
    occurrencesBefore: 3,
  };

  /** Runs a split whose truncate fails with `failure`; says what happened. */
  async function truncateFailsWith(
    failure: unknown,
    removal: () => Promise<unknown> = async () => undefined,
  ) {
    const removeTail = vi.fn(removal);
    let caught: unknown = null;
    let written: unknown = null;
    try {
      written = await writeSeriesSplit(
        {
          createTail: async () => ({ id: 'tail' }),
          truncate: async () => {
            throw failure;
          },
          removeTail,
        },
        plan,
      );
    } catch (err) {
      caught = err;
    }
    return { caught, written, removeTail };
  }

  it('creates the tail first, then truncates (136)', async () => {
    // The other order lost data: a truncate that landed and a tail that did
    // not left the series ending at the cutoff, and putting the head back
    // restored only its rule.
    const order: string[] = [];
    const created = await writeSeriesSplit(
      {
        createTail: async (rec) => {
          order.push(`create:${rec.rrule}`);
          return { id: 'tail' };
        },
        truncate: async (rule) => {
          order.push(`truncate:${rule}`);
        },
        removeTail: async () => {
          order.push('remove');
        },
      },
      plan,
    );
    expect(created).toEqual({ tail: { id: 'tail' }, headCut: 'done' });
    expect(order).toEqual([
      'create:FREQ=WEEKLY;COUNT=7',
      'truncate:FREQ=WEEKLY;UNTIL=20260824T075959Z',
    ]);
  });

  it('writes nothing more when the tail cannot be created', async () => {
    const truncate = vi.fn(async () => undefined);
    const removeTail = vi.fn(async () => undefined);
    let caught: unknown = null;
    try {
      await writeSeriesSplit(
        {
          createTail: async () => {
            throw new Error('the server said no');
          },
          truncate,
          removeTail,
        },
        plan,
      );
    } catch (err) {
      caught = err;
    }
    expect((caught as Error).message).toBe('the server said no');
    expect(truncate).not.toHaveBeenCalled();
    expect(removeTail).not.toHaveBeenCalled();
    // The calendar is as it was.
    expect(seriesMaybeShownTwice(caught)).toBe(false);
  });

  it('deletes the tail again when the truncate was refused', async () => {
    // A conflict, a missing right: the truncate changed nothing, so deleting
    // the series just created leaves the calendar exactly as it was.
    for (const refused of [
      coded('conflict', 'the series changed on the server'),
      coded('forbidden', 'read-only calendar'),
      coded('invalid_input', 'no rule'),
      // A refusal token says so whatever code carried it: an unknown identity
      // travels as a network error, but no request went out.
      coded('network', 'identity-unknown: me@example.org'),
    ]) {
      const { caught, removeTail } = await truncateFailsWith(refused);
      expect(caught, refused.message).toBe(refused);
      expect(removeTail, refused.message).toHaveBeenCalledWith({ id: 'tail' });
      expect(seriesMaybeShownTwice(caught), refused.message).toBe(false);
    }
  });

  it('keeps both when the truncate may have landed, and counts the tail written (144, 145)', async () => {
    // The answer may be lost after the provider cut the series. Deleting the
    // tail then would leave the series ending at the cutoff after all; and
    // throwing would invite the caller to write the tail a second time.
    for (const unsure of [
      coded('network', 'connection reset'),
      coded('protocol', 'unreadable answer'),
      coded('internal', 'cache write failed'),
      new Error('Call to function has been rejected.'),
      'offline',
    ]) {
      const { caught, written, removeTail } = await truncateFailsWith(unsure);
      expect(caught).toBeNull();
      expect(removeTail).not.toHaveBeenCalled();
      expect(written).toEqual({ tail: { id: 'tail' }, headCut: 'unsure', failure: unsure });
    }
  });

  it('marks it when the tail cannot be deleted again', async () => {
    const refused = coded('conflict', 'the series changed on the server');
    const { caught, removeTail } = await truncateFailsWith(refused, async () => {
      throw new Error('and the delete failed as well');
    });
    expect(removeTail).toHaveBeenCalledOnce();
    // The failure the user is waiting to hear about, not the repair's.
    expect(caught).toBe(refused);
    expect(seriesMaybeShownTwice(caught)).toBe(true);
  });

  it('marks a refusal that is no object on an error of its own', async () => {
    const { caught } = await truncateFailsWith('server-refused: quota', async () => {
      throw new Error('and the delete failed as well');
    });
    expect((caught as Error).message).toBe('server-refused: quota');
    expect(seriesMaybeShownTwice(caught)).toBe(true);
  });
});

describe('cutoffDay', () => {
  it('names the day the occurrence is on, in the reader language', () => {
    // Half past midnight local time: the UTC day may be the one before.
    const iso = new Date(2026, 7, 24, 0, 30).toISOString();
    expect(cutoffDay(iso, 'de')).toBe('24. August 2026');
    expect(cutoffDay(iso, 'en')).toBe('August 24, 2026');
  });

  it('keeps what it cannot read', () => {
    expect(cutoffDay('not a date', 'de')).toBe('not a date');
  });
});

// The composition the two carry dialogs run for `scope: 'future'`. It is UI
// code in both of them, so this is where the SEQUENCE is pinned down: the copy
// is cut at its OWN next occurrence, the tail carries only what changed, and
// everything that belongs to the copy stays with it.
describe('carrying a future edit to another copy', () => {
  /** The copy in the private calendar: same appointment, its own series, and a
   *  reminder that is the whole reason the copy exists. */
  const copy = {
    id: 'ev-private',
    calendar_id: 'private',
    title: 'Wochenplanung',
    start: '2026-08-03T08:00:00.000Z',
    end: '2026-08-03T09:00:00.000Z',
    all_day: false,
    location: 'Raum 3',
    description: null,
    reminders: ['-PT30M'],
    color_label: 'blue',
    recurrence: {
      rrule: 'FREQ=WEEKLY;COUNT=10',
      exceptions: [] as string[],
      tzid: null as string | null,
    },
  };

  /** What the two dialogs do, in the order they do it. */
  async function carryFuture(
    current: Omit<typeof copy, 'recurrence'> & {
      recurrence: (typeof copy)['recurrence'] | null;
    },
    cutoffIso: string,
    before: {
      title: string;
      start: string;
      end: string;
      all_day: boolean;
      location: string | null;
      description: string | null;
    },
    after: {
      title: string;
      start: string;
      end: string;
      all_day: boolean;
      location: string | null;
      description: string | null;
    },
    changed: ('title' | 'start' | 'end' | 'all_day' | 'location' | 'description')[],
    /** The copy's own provider-kept rows, read first, as the dialogs do. */
    rows: RecurringRow[] = [],
  ): Promise<{
    /** Where this copy is cut. */
    anchorIso: string;
    /** The row the tail (or the single event) is written with. */
    row: typeof current;
    /** False when the copy is updated in place, not split: a single event, or
     *  a series with nothing before its cut point (decision 118). */
    split: boolean;
    headRule: string | null;
    tailRule: string | null;
  } | null> {
    const anchorIso = firstOccurrenceFrom(current, cutoffIso, rows);
    if (anchorIso == null) return null;
    const row = futureCarryRow(current, anchorIso, before, after, changed);
    // These fixtures all carry readable instants; a null here would be the
    // test's own mistake, not the rule's.
    if (row == null) throw new Error('the fixture has an unreadable cut point');
    const plan = planSeriesSplit(current, anchorIso, rows);
    if (plan == null) {
      return { anchorIso, row, split: false, headRule: null, tailRule: null };
    }
    if (plan.kind === 'whole') {
      return { anchorIso, row, split: false, headRule: null, tailRule: plan.tail.rrule };
    }
    let headRule: string | null = null;
    let tailRule: string | null = null;
    await writeSeriesSplit(
      {
        truncate: async (rule) => {
          headRule = rule;
        },
        createTail: async (recurrence) => {
          tailRule = recurrence.rrule;
          return { id: 'ev-private-tail' };
        },
        removeTail: async () => undefined,
      },
      plan,
    );
    return { anchorIso, row, split: true, headRule, tailRule };
  }

  /** The anchor's occurrence as it stood before the edit. */
  const stood = {
    title: 'Wochenplanung',
    start: '2026-08-24T08:00:00.000Z',
    end: '2026-08-24T09:00:00.000Z',
    all_day: false,
    location: 'Raum 3',
    description: null,
  };
  /** …and after it was moved an hour later. */
  const movedAnHourLater = {
    ...stood,
    start: '2026-08-24T09:00:00.000Z',
    end: '2026-08-24T10:00:00.000Z',
  };

  it('cuts a copy at its occurrence changed in Outlook, not a week later (141)', async () => {
    // The copy's own cut-day occurrence was changed in Outlook: its slot is
    // among the exceptions, and a row stands in it. Read with the rows, the
    // copy is cut there, and that day carries the change.
    const changedThere = {
      ...copy,
      recurrence: { ...copy.recurrence, exceptions: ['2026-08-24T08:00:00.000Z'] },
    };
    const moved: RecurringRow = {
      id: 'ev-private::rid::2026-08-24T08:00:00Z',
      start: '2026-08-24T10:00:00.000Z',
      end: '2026-08-24T11:00:00.000Z',
      all_day: false,
      recurrence: null,
    };
    const result = await carryFuture(
      changedThere,
      '2026-08-24T08:00:00.000Z',
      stood,
      movedAnHourLater,
      ['start', 'end'],
      [moved],
    );
    expect(result?.anchorIso).toBe('2026-08-24T08:00:00.000Z');
    expect(result?.headRule).toContain('UNTIL=20260824T075959Z');
  });

  it('cuts the copy series and hands the tail the change, not the copy own life', async () => {
    const result = await carryFuture(
      copy,
      '2026-08-24T08:00:00.000Z',
      stood,
      movedAnHourLater,
      ['start', 'end'],
    );
    expect(result?.split).toBe(true);
    expect(result?.headRule).toContain('UNTIL=20260824T075959Z');
    expect(result?.tailRule).toBe('FREQ=WEEKLY;COUNT=7');
    // The move travelled…
    expect(result?.row.start).toBe('2026-08-24T09:00:00.000Z');
    // …and everything that makes this copy a copy stayed.
    expect(result?.row.reminders).toEqual(['-PT30M']);
    expect(result?.row.color_label).toBe('blue');
    expect(result?.row.calendar_id).toBe('private');
  });

  it('rewrites a copy in place when nothing of it comes before its cut point', async () => {
    // The copy starts on the day the anchor was cut: cutting it would leave a
    // head that ends before it starts, hidden and still ringing (118).
    const startsAtTheCut = {
      ...copy,
      start: '2026-08-24T08:00:00.000Z',
      end: '2026-08-24T09:00:00.000Z',
    };
    const result = await carryFuture(
      startsAtTheCut,
      '2026-08-24T08:00:00.000Z',
      stood,
      movedAnHourLater,
      ['start', 'end'],
    );
    expect(result?.split).toBe(false);
    expect(result?.headRule).toBeNull();
    expect(result?.tailRule).toBe('FREQ=WEEKLY;COUNT=10');
    expect(result?.row.start).toBe('2026-08-24T09:00:00.000Z');
  });

  it('cuts a differently-patterned copy at its own next occurrence', async () => {
    // The private copy runs fortnightly: it has nothing on the day the work
    // series was split, so "and all following" starts at ITS next one.
    const fortnightly = {
      ...copy,
      recurrence: { ...copy.recurrence, rrule: 'FREQ=WEEKLY;INTERVAL=2;COUNT=10' },
    };
    const result = await carryFuture(
      fortnightly,
      '2026-08-24T08:00:00.000Z',
      stood,
      { ...stood, title: 'Wochenplanung kurz' },
      ['title'],
    );
    expect(result?.anchorIso).toBe('2026-08-31T08:00:00.000Z');
    // A title-only edit must not drag the copy to the anchor's instants.
    expect(result?.row.start).toBe('2026-08-31T08:00:00.000Z');
    expect(result?.row.title).toBe('Wochenplanung kurz');
    expect(result?.headRule).toContain('UNTIL=20260831T075959Z');
  });

  it('moves a differently-patterned copy BY the edit, not TO the anchor instant', async () => {
    // The bug this pins down: the head was cut at the copy's own occurrence
    // (08-31) while the tail was created at the anchor's new instant (08-24
    // 09:00). The copy lost its real 08-31 appointment, gained one on a day it
    // never had, and every occurrence after it fell a week out of phase.
    const fortnightly = {
      ...copy,
      recurrence: { ...copy.recurrence, rrule: 'FREQ=WEEKLY;INTERVAL=2;COUNT=10' },
    };
    const result = await carryFuture(
      fortnightly,
      '2026-08-24T08:00:00.000Z',
      stood,
      movedAnHourLater,
      ['start', 'end'],
    );
    // Its own cut point, one hour later — the move, not the instant.
    expect(result?.anchorIso).toBe('2026-08-31T08:00:00.000Z');
    expect(result?.row.start).toBe('2026-08-31T09:00:00.000Z');
    expect(result?.row.end).toBe('2026-08-31T10:00:00.000Z');
    // And the head ends just before the same point, so the halves meet.
    expect(result?.headRule).toContain('UNTIL=20260831T075959Z');
  });

  it('never writes an end before its start when only the duration changed', async () => {
    // A duration-only edit puts just 'end' in `changed`. Taking the anchor's
    // end verbatim gave a misaligned copy an end a week BEFORE its start.
    const fortnightly = {
      ...copy,
      recurrence: { ...copy.recurrence, rrule: 'FREQ=WEEKLY;INTERVAL=2;COUNT=10' },
    };
    const result = await carryFuture(
      fortnightly,
      '2026-08-24T08:00:00.000Z',
      stood,
      { ...stood, end: '2026-08-24T08:30:00.000Z' },
      ['end'],
    );
    expect(result?.row.start).toBe('2026-08-31T08:00:00.000Z');
    expect(result?.row.end).toBe('2026-08-31T08:30:00.000Z');
    expect(new Date(result!.row.end).getTime()).toBeGreaterThan(
      new Date(result!.row.start).getTime(),
    );
  });

  it('moves an all-day copy in whole days and keeps its length', async () => {
    // The anchor is timed and was moved an hour; an all-day copy cannot be, and
    // an hour-long all-day event is not a thing either.
    const localMidnight = (y: number, m: number, d: number) =>
      new Date(y, m - 1, d).toISOString();
    const allDayCopy = {
      ...copy,
      all_day: true,
      start: localMidnight(2026, 8, 3),
      end: localMidnight(2026, 8, 4),
    };
    const result = await carryFuture(
      allDayCopy,
      localMidnight(2026, 8, 24),
      stood,
      movedAnHourLater,
      ['start', 'end'],
    );
    expect(result?.row.start).toBe(localMidnight(2026, 8, 24));
    expect(result?.row.end).toBe(localMidnight(2026, 8, 25));
  });

  it('leaves a copy alone when its series ends before the cutoff', async () => {
    // Nothing to carry — and the dialogs report that rather than counting it
    // as done, because a copy that silently kept its old shape is the
    // contradiction the group exists to prevent.
    const short = {
      ...copy,
      recurrence: { ...copy.recurrence, rrule: 'FREQ=WEEKLY;COUNT=2' },
    };
    expect(
      await carryFuture(short, '2026-08-24T08:00:00.000Z', stood, movedAnHourLater, [
        'start',
      ]),
    ).toBeNull();
  });

  it('updates a copy that is a single event instead of splitting it', async () => {
    // One occurrence, so "this and all following" is that one.
    const single = { ...copy, recurrence: null };
    const result = await carryFuture(
      single,
      '2026-08-03T08:00:00.000Z',
      { ...stood, start: '2026-08-03T08:00:00.000Z', end: '2026-08-03T09:00:00.000Z' },
      { ...stood, start: '2026-08-03T09:00:00.000Z', end: '2026-08-03T10:00:00.000Z' },
      ['start', 'end'],
    );
    expect(result?.split).toBe(false);
    expect(result?.row.start).toBe('2026-08-03T09:00:00.000Z');
    expect(result?.row.reminders).toEqual(['-PT30M']);
  });
});

describe('the slot rule, asked once per series', () => {
  it('asks the core for a series clock a handful of times, not once per comparison', () => {
    // Which clock a series is read on crosses the WASM or FFI boundary. A
    // year of a daily series against sixty rows of its own asked it for every
    // comparison — tens of thousands of calls on the phone.
    let asked = 0;
    installSeriesClockRules({
      seriesClockZone: seriesClockZoneThroughCore,
      canonicalZone: canonicalZoneThroughCore,
      expansionClock: (allDay, tzid) => {
        asked += 1;
        return expansionClockThroughCore(allDay, tzid);
      },
    });
    try {
      const daily = {
        ...weekly,
        recurrence: { ...weekly.recurrence, rrule: 'FREQ=DAILY' },
      };
      const rows = Array.from({ length: 60 }, (_, i) =>
        rowOf(new Date(Date.parse(weekly.start) + i * 7 * 86_400_000).toISOString()),
      );
      const range = { start: new Date('2026-08-01T00:00:00Z'), end: new Date('2027-08-01T00:00:00Z') };
      asked = 0;
      expandAll([daily, ...rows], range);
      expect(asked).toBeLessThan(10);
      // The split's reading compares every exception with every row, and
      // every cancelled row with every slot found so far.
      const excepted = {
        ...daily,
        recurrence: {
          ...daily.recurrence,
          exceptions: rows.map((row) => row.id.split('::rid::')[1]),
        },
      };
      const cancelled = rows.map((_row, i) =>
        rowOf(new Date(Date.parse(weekly.start) + (i * 7 + 3) * 86_400_000).toISOString(), undefined, {
          cancelled: true,
        }),
      );
      asked = 0;
      expect(deletedSlots(excepted, [...rows, ...cancelled])).toHaveLength(60);
      expect(asked).toBeLessThan(10);
    } finally {
      installSeriesClockRules({
        seriesClockZone: seriesClockZoneThroughCore,
        canonicalZone: canonicalZoneThroughCore,
        expansionClock: expansionClockThroughCore,
      });
    }
  });
});
