import { describe, expect, it } from 'vitest';

import { allDayReminderDays } from '@aperio/shared';

describe('allDayReminderDays', () => {
  it('a single all-day day (start → next midnight) is one day', () => {
    expect(
      allDayReminderDays('2026-06-24T00:00:00Z', '2026-06-25T00:00:00Z'),
    ).toBe(1);
  });

  it('a three-day all-day event spans three days (end is exclusive)', () => {
    // 24, 25, 26 June → end = 27 June 00:00.
    expect(
      allDayReminderDays('2026-06-24T00:00:00Z', '2026-06-27T00:00:00Z'),
    ).toBe(3);
  });

  it('is robust to local-midnight-as-UTC anchoring (Berlin, +2)', () => {
    // 24 June local midnight = 23 June 22:00Z; 27 June local midnight = 26 June
    // 22:00Z. The ms delta is still exactly three days.
    expect(
      allDayReminderDays('2026-06-23T22:00:00Z', '2026-06-26T22:00:00Z'),
    ).toBe(3);
  });

  it('degrades to one day for an unparseable or non-positive span', () => {
    expect(allDayReminderDays('nonsense', '2026-06-25T00:00:00Z')).toBe(1);
    expect(
      allDayReminderDays('2026-06-25T00:00:00Z', '2026-06-24T00:00:00Z'),
    ).toBe(1);
  });
});
