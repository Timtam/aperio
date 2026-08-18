import { describe, expect, it } from 'vitest';

import { quickDates } from '@aperio/shared';

/** Local `YYYY-MM-DD` for a fixed local date. */
const at = (y: number, m: number, d: number) => new Date(y, m - 1, d);
const keys = (today: Date, weekStartsOn = 1) =>
  Object.fromEntries(quickDates(today, weekStartsOn).map((q) => [q.id, q.dayKey]));

describe('quickDates', () => {
  it('offers today and tomorrow', () => {
    // Wednesday, 19 August 2026.
    const k = keys(at(2026, 8, 19));
    expect(k.today).toBe('2026-08-19');
    expect(k.tomorrow).toBe('2026-08-20');
  });

  it('points the weekend at the coming Saturday', () => {
    expect(keys(at(2026, 8, 19)).weekend).toBe('2026-08-22'); // Wed → Sat
    expect(keys(at(2026, 8, 21)).weekend).toBe('2026-08-22'); // Fri → Sat
  });

  it('stays on today when today IS Saturday', () => {
    expect(keys(at(2026, 8, 22)).weekend).toBe('2026-08-22');
  });

  it('reaches six days out on a Sunday', () => {
    // "Next weekend", said on a Sunday evening, does not mean today.
    expect(keys(at(2026, 8, 23)).weekend).toBe('2026-08-29');
  });

  it('follows the week start for "next week"', () => {
    // Wednesday → the following Monday, or the following Sunday.
    expect(keys(at(2026, 8, 19), 1).nextWeek).toBe('2026-08-24');
    expect(keys(at(2026, 8, 19), 0).nextWeek).toBe('2026-08-23');
  });

  it('is a whole week away when today IS the start of the week', () => {
    // Standing on Monday, "next week" is next Monday — not today.
    expect(keys(at(2026, 8, 24), 1).nextWeek).toBe('2026-08-31');
  });

  it('crosses a month and a year boundary', () => {
    expect(keys(at(2026, 8, 31)).tomorrow).toBe('2026-09-01');
    expect(keys(at(2026, 12, 31)).tomorrow).toBe('2027-01-01');
  });
});
