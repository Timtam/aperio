import { describe, expect, it } from 'vitest';
import { localDateKey } from './dateKey';

describe('localDateKey', () => {
  it('returns the local YYYY-MM-DD', () => {
    const d = new Date(2026, 4, 19, 10, 0, 0); // 19 May 2026, 10:00 local
    expect(localDateKey(d)).toBe('2026-05-19');
  });

  it('pads month and day', () => {
    const d = new Date(2026, 0, 5, 12, 0, 0);
    expect(localDateKey(d)).toBe('2026-01-05');
  });

  it('does not shift the day across UTC, regardless of host timezone', () => {
    // Whatever timezone the test host happens to run in, the local
    // wall-clock day of a Date(...) constructed from local components
    // must be preserved.
    const d = new Date(2026, 11, 31, 23, 30, 0); // 31 Dec, 23:30 local
    expect(localDateKey(d)).toBe('2026-12-31');
  });
});
