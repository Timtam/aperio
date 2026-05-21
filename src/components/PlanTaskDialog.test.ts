import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';

// We're only testing the date-helper logic; React rendering with a
// real Modal needs the full provider stack which is overkill here.
// The helpers are pure functions over `new Date()`, so freezing the
// clock with vi.useFakeTimers covers everything we care about
// (off-by-one bugs around midnight, DST transitions, Sunday→Monday
// edge case in "next Monday").

// Re-export the helpers as part of the component file's public-ish
// surface — we treat them as module-scoped utilities and call them
// directly through a `__test` re-export.
import {
  __test as planTaskHelpers,
} from './PlanTaskDialog';

describe('PlanTaskDialog date helpers', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('isoToday returns the current local calendar day', () => {
    // Frozen at 14:00 on a Wednesday in May; the date should round
    // to the local day regardless of UTC offset.
    vi.setSystemTime(new Date(2026, 4, 20, 14, 0, 0)); // 2026-05-20
    expect(planTaskHelpers.isoToday()).toBe('2026-05-20');
  });

  it('isoTomorrow advances exactly one local day', () => {
    vi.setSystemTime(new Date(2026, 4, 20, 23, 30, 0));
    expect(planTaskHelpers.isoTomorrow()).toBe('2026-05-21');
  });

  it('isoNextMonday lands on Monday from any weekday', () => {
    // 2026-05-20 is a Wednesday — next Monday is 2026-05-25.
    vi.setSystemTime(new Date(2026, 4, 20, 9, 0, 0));
    expect(planTaskHelpers.isoNextMonday()).toBe('2026-05-25');

    // 2026-05-24 is a Sunday — next Monday is the very next day.
    vi.setSystemTime(new Date(2026, 4, 24, 9, 0, 0));
    expect(planTaskHelpers.isoNextMonday()).toBe('2026-05-25');

    // 2026-05-25 IS a Monday — "next Monday" should mean a full week
    // out (the current day isn't "next"), not today.
    vi.setSystemTime(new Date(2026, 4, 25, 9, 0, 0));
    expect(planTaskHelpers.isoNextMonday()).toBe('2026-06-01');
  });

  it('isoTomorrow handles month boundaries', () => {
    // Last day of May → tomorrow is June 1.
    vi.setSystemTime(new Date(2026, 4, 31, 12, 0, 0));
    expect(planTaskHelpers.isoTomorrow()).toBe('2026-06-01');
  });

  it('isoTomorrow handles year boundaries', () => {
    // Last day of the year.
    vi.setSystemTime(new Date(2026, 11, 31, 12, 0, 0));
    expect(planTaskHelpers.isoTomorrow()).toBe('2027-01-01');
  });
});
