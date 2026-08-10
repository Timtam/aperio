import { describe, expect, it } from 'vitest';

import { backlogWeeks, splitDeadlinesByWeek } from '@aperio/shared';

// 2026-08-10 is a Monday, 2026-08-12 a Wednesday.
const MONDAY = '2026-08-10';
const WEDNESDAY = '2026-08-12';

describe('backlogWeeks', () => {
  it('runs Monday to Sunday when the week starts on Monday', () => {
    expect(backlogWeeks(WEDNESDAY, 1)).toEqual({
      thisWeekStart: '2026-08-10',
      thisWeekEnd: '2026-08-16',
      nextWeekStart: '2026-08-17',
      nextWeekEnd: '2026-08-23',
    });
  });

  it('runs Sunday to Saturday when the week starts on Sunday', () => {
    // The same Wednesday, a different setting: the window shifts by one day,
    // and Sunday the 16th now belongs to NEXT week rather than this one.
    expect(backlogWeeks(WEDNESDAY, 0)).toEqual({
      thisWeekStart: '2026-08-09',
      thisWeekEnd: '2026-08-15',
      nextWeekStart: '2026-08-16',
      nextWeekEnd: '2026-08-22',
    });
  });

  it('puts the first day of the week in its own week, not the previous one', () => {
    // The off-by-one that a naive modulo gets wrong: on the start day itself
    // the offset is zero, so the week must begin today.
    expect(backlogWeeks(MONDAY, 1).thisWeekStart).toBe(MONDAY);
    expect(backlogWeeks('2026-08-09', 0).thisWeekStart).toBe('2026-08-09');
  });

  it('crosses a month and a year boundary without drifting', () => {
    // Thursday 2026-12-31, Monday-start week: Mon 28 Dec … Sun 3 Jan.
    expect(backlogWeeks('2026-12-31', 1)).toEqual({
      thisWeekStart: '2026-12-28',
      thisWeekEnd: '2027-01-03',
      nextWeekStart: '2027-01-04',
      nextWeekEnd: '2027-01-10',
    });
  });

  it('is a calendar week, not seven days from now', () => {
    // Sunday of a Monday-start week: "this week" has one day left. Seven days
    // from now would run to the following Saturday and never say where the
    // week ends.
    const weeks = backlogWeeks('2026-08-16', 1);
    expect(weeks.thisWeekEnd).toBe('2026-08-16');
    expect(weeks.nextWeekStart).toBe('2026-08-17');
  });
});

describe('splitDeadlinesByWeek', () => {
  const weeks = backlogWeeks(WEDNESDAY, 1); // 10.–16. / 17.–23.
  const task = (id: string, deadline_date: string | null) => ({ id, deadline_date });

  it('sorts each task into the week its deadline falls in', () => {
    const split = splitDeadlinesByWeek(
      [
        task('a', '2026-08-16'),
        task('b', '2026-08-17'),
        task('c', '2026-08-23'),
        task('d', '2026-08-24'),
      ],
      weeks,
    );
    expect(split.thisWeek.map((x) => x.id)).toEqual(['a']);
    expect(split.nextWeek.map((x) => x.id)).toEqual(['b', 'c']);
    expect(split.later.map((x) => x.id)).toEqual(['d']);
  });

  it('keeps an overdue deadline with this week', () => {
    // The most urgent thing the rail holds. The date sort puts it at the very
    // top of the first section; the tail would bury it.
    const split = splitDeadlinesByWeek(
      [task('last-tuesday', '2026-08-04'), task('friday', '2026-08-14')],
      weeks,
    );
    expect(split.thisWeek.map((x) => x.id)).toEqual(['last-tuesday', 'friday']);
    expect(split.later).toEqual([]);
  });

  it('preserves the order it was handed', () => {
    // The rail sorts by date, then priority, then creation before calling this,
    // and every bucket has to keep that.
    const split = splitDeadlinesByWeek(
      [task('first', '2026-08-12'), task('second', '2026-08-12')],
      weeks,
    );
    expect(split.thisWeek.map((x) => x.id)).toEqual(['first', 'second']);
  });

  it('ignores a task without a deadline', () => {
    expect(splitDeadlinesByWeek([task('none', null)], weeks)).toEqual({
      thisWeek: [],
      nextWeek: [],
      later: [],
    });
  });
});
