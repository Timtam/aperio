import { describe, expect, it, vi } from 'vitest';

import {
  firstOccurrenceFrom,
  occurrenceCarryRow,
  planSeriesSplit,
  writeSeriesSplit,
  type SeriesSplitPlan,
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

describe('planSeriesSplit', () => {
  it('ends the head before the cutoff and gives the tail what is left', () => {
    // Split at the fourth occurrence: three stay, seven move.
    const plan = planSeriesSplit(weekly, '2026-08-24T08:00:00.000Z');
    expect(plan).not.toBeNull();
    expect(plan?.occurrencesBefore).toBe(3);
    expect(plan?.headRule).toContain('UNTIL=20260824T075959Z');
    expect(plan?.headRule).not.toContain('COUNT');
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
    const plan = planSeriesSplit(withHole, '2026-08-24T08:00:00.000Z');
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
    const plan = planSeriesSplit(withHoles, '2026-08-24T08:00:00.000Z');
    expect(plan?.tail.exceptions).toEqual(['2026-09-07T08:00:00.000Z']);
  });

  it('carries the zone verbatim, floating included', () => {
    const zoned = {
      ...weekly,
      recurrence: { ...weekly.recurrence, tzid: 'Europe/Berlin' },
    };
    expect(planSeriesSplit(zoned, '2026-08-24T08:00:00.000Z')?.tail.tzid).toBe(
      'Europe/Berlin',
    );
    // A floating series stays floating: stamping only the tail would make the
    // two halves expand an hour apart across a DST boundary.
    expect(planSeriesSplit(weekly, '2026-08-24T08:00:00.000Z')?.tail.tzid).toBeNull();
  });

  it('leaves an open-ended series open', () => {
    const open = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY' },
    };
    const plan = planSeriesSplit(open, '2026-08-24T08:00:00.000Z');
    expect(plan?.tail.rrule).toBe('FREQ=WEEKLY');
    expect(plan?.headRule).toContain('UNTIL=');
  });

  it('emits a date-only UNTIL for an all-day series', () => {
    // A DATE-valued series needs a DATE-valued UNTIL, or strict providers drop
    // the rule outright.
    const allDay = {
      ...weekly,
      all_day: true,
      start: '2026-08-03T00:00:00.000Z',
      end: '2026-08-04T00:00:00.000Z',
    };
    const plan = planSeriesSplit(allDay, '2026-08-24T00:00:00.000Z');
    expect(plan?.headRule).toContain('UNTIL=20260823');
    expect(plan?.headRule).not.toContain('UNTIL=20260823T');
  });

  it('refuses an event that carries no rule', () => {
    // The caller must not fall through to a whole-series edit on this: that
    // moves every occurrence, which is what the scope question prevents.
    const single = { ...weekly, recurrence: null };
    expect(planSeriesSplit(single, '2026-08-24T08:00:00.000Z')).toBeNull();
  });
});

describe('firstOccurrenceFrom', () => {
  it('is the cutoff itself when the series has an occurrence there', () => {
    expect(firstOccurrenceFrom(weekly, '2026-08-24T08:00:00.000Z')).toBe(
      '2026-08-24T08:00:00.000Z',
    );
  });

  it('is the copy own next occurrence when the patterns differ', () => {
    // The other copy of the same appointment runs FORTNIGHTLY, so it has
    // nothing on the cutoff day — its "and all following" starts a week later.
    const fortnightly = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY;INTERVAL=2;COUNT=10' },
    };
    expect(firstOccurrenceFrom(fortnightly, '2026-08-24T08:00:00.000Z')).toBe(
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
    expect(firstOccurrenceFrom(everyThreeYears, '2026-08-24T08:00:00.000Z')).toBe(
      '2029-08-03T08:00:00.000Z',
    );
  });

  it('is nothing when the series has already ended', () => {
    const short = {
      ...weekly,
      recurrence: { ...weekly.recurrence, rrule: 'FREQ=WEEKLY;COUNT=2' },
    };
    expect(firstOccurrenceFrom(short, '2026-08-24T08:00:00.000Z')).toBeNull();
  });

  it('treats a single event as its own only occurrence', () => {
    const single = { ...weekly, recurrence: null };
    expect(firstOccurrenceFrom(single, '2026-08-03T08:00:00.000Z')).toBe(
      '2026-08-03T08:00:00.000Z',
    );
    expect(firstOccurrenceFrom(single, '2026-08-24T08:00:00.000Z')).toBeNull();
  });
});

describe('writeSeriesSplit', () => {
  const plan: SeriesSplitPlan = {
    headRule: 'FREQ=WEEKLY;UNTIL=20260824T075959Z',
    tail: { rrule: 'FREQ=WEEKLY;COUNT=7', exceptions: [], tzid: null },
    occurrencesBefore: 3,
  };

  it('truncates first, then creates the tail', async () => {
    const order: string[] = [];
    const created = await writeSeriesSplit(
      {
        truncate: async (rule) => {
          order.push(`truncate:${rule}`);
        },
        createTail: async (rec) => {
          order.push(`create:${rec.rrule}`);
          return { id: 'tail' };
        },
        restore: async () => {
          order.push('restore');
        },
      },
      plan,
    );
    expect(created).toEqual({ id: 'tail' });
    expect(order).toEqual([
      'truncate:FREQ=WEEKLY;UNTIL=20260824T075959Z',
      'create:FREQ=WEEKLY;COUNT=7',
    ]);
  });

  it('puts the master back when the tail cannot be created', async () => {
    // Without this the series simply ENDS at the cutoff: every appointment from
    // there on is gone, and nothing on screen says so.
    const restore = vi.fn(async () => undefined);
    await expect(
      writeSeriesSplit(
        {
          truncate: async () => undefined,
          createTail: async () => {
            throw new Error('the server said no');
          },
          restore,
        },
        plan,
      ),
    ).rejects.toThrow('the server said no');
    expect(restore).toHaveBeenCalledOnce();
  });

  it('reports the original failure even when the restore fails too', async () => {
    // The restore failing is worth nothing to the user; the write that failed
    // is what they are waiting to hear about.
    await expect(
      writeSeriesSplit(
        {
          truncate: async () => undefined,
          createTail: async () => {
            throw new Error('the server said no');
          },
          restore: async () => {
            throw new Error('and the restore failed as well');
          },
        },
        plan,
      ),
    ).rejects.toThrow('the server said no');
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
    after: {
      title: string;
      start: string;
      end: string;
      all_day: boolean;
      location: string | null;
      description: string | null;
    },
    changed: ('title' | 'start' | 'end' | 'all_day' | 'location' | 'description')[],
  ): Promise<{
    /** Where this copy is cut. */
    anchorIso: string;
    /** The row the tail (or the single event) is written with. */
    row: typeof current;
    /** False when the copy is a single event: updated in place, not split. */
    split: boolean;
    headRule: string | null;
    tailRule: string | null;
  } | null> {
    const anchorIso = firstOccurrenceFrom(current, cutoffIso);
    if (anchorIso == null) return null;
    const row = occurrenceCarryRow(current, anchorIso, after, changed);
    const plan = planSeriesSplit(current, anchorIso);
    if (plan == null) {
      return { anchorIso, row, split: false, headRule: null, tailRule: null };
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
        restore: async () => undefined,
      },
      plan,
    );
    return { anchorIso, row, split: true, headRule, tailRule };
  }

  const movedAnHourLater = {
    title: 'Wochenplanung',
    start: '2026-08-24T09:00:00.000Z',
    end: '2026-08-24T10:00:00.000Z',
    all_day: false,
    location: 'Raum 3',
    description: null,
  };

  it('cuts the copy series and hands the tail the change, not the copy own life', async () => {
    const result = await carryFuture(copy, '2026-08-24T08:00:00.000Z', movedAnHourLater, [
      'start',
      'end',
    ]);
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
      { ...movedAnHourLater, title: 'Wochenplanung kurz' },
      ['title'],
    );
    expect(result?.anchorIso).toBe('2026-08-31T08:00:00.000Z');
    // A title-only edit must not drag the copy to the anchor's instants.
    expect(result?.row.start).toBe('2026-08-31T08:00:00.000Z');
    expect(result?.row.title).toBe('Wochenplanung kurz');
    expect(result?.headRule).toContain('UNTIL=20260831T075959Z');
  });

  it('leaves a copy alone when its series ends before the cutoff', async () => {
    // Nothing to carry — and the dialogs report that rather than counting it
    // as done, because a copy that silently kept its old shape is the
    // contradiction the group exists to prevent.
    const short = {
      ...copy,
      recurrence: { ...copy.recurrence, rrule: 'FREQ=WEEKLY;COUNT=2' },
    };
    expect(await carryFuture(short, '2026-08-24T08:00:00.000Z', movedAnHourLater, ['start']))
      .toBeNull();
  });

  it('updates a copy that is a single event instead of splitting it', async () => {
    // One occurrence, so "this and all following" is that one.
    const single = { ...copy, recurrence: null };
    const result = await carryFuture(single, '2026-08-03T08:00:00.000Z', movedAnHourLater, [
      'start',
      'end',
    ]);
    expect(result?.split).toBe(false);
    expect(result?.row.start).toBe('2026-08-24T09:00:00.000Z');
    expect(result?.row.reminders).toEqual(['-PT30M']);
  });
});
