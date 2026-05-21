import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { shouldFireToday } from './useCurrentDayKey';

describe('shouldFireToday', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Local time 09:30. Date is 2026-05-20 (Wed). The `now` argument
    // defaults to `new Date()`, so this also pins what the helper
    // sees as "current local time".
    vi.setSystemTime(new Date(2026, 4, 20, 9, 30, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  describe('app-start trigger', () => {
    it('fires when nothing has fired yet', () => {
      expect(shouldFireToday('app-start', null, '2026-05-20')).toBe(true);
    });

    it('never re-fires once a day key was recorded', () => {
      // Mount-once semantics: even when the date rolls over the next
      // day, app-start mode stays silent.
      expect(shouldFireToday('app-start', '2026-05-19', '2026-05-20')).toBe(
        false,
      );
    });
  });

  describe('HH:MM trigger', () => {
    it('fires when no day key was recorded and threshold passed', () => {
      // 00:00 threshold is always passed; the first launch of the day
      // gets the review.
      expect(shouldFireToday('00:00', null, '2026-05-20')).toBe(true);
    });

    it('does not fire twice on the same day', () => {
      // Already fired today — the gate keeps quiet for the rest of
      // the day regardless of the threshold.
      expect(shouldFireToday('00:00', '2026-05-20', '2026-05-20')).toBe(
        false,
      );
    });

    it('re-fires when the date rolls over', () => {
      // Fired yesterday, today is a new day → eligible again.
      expect(shouldFireToday('00:00', '2026-05-19', '2026-05-20')).toBe(true);
    });

    it('waits until the configured morning threshold', () => {
      // Now is 09:30. An 08:00 trigger should have fired already.
      expect(shouldFireToday('08:00', null, '2026-05-20')).toBe(true);
      // An 12:00 trigger is still in the future; stay quiet.
      expect(shouldFireToday('12:00', null, '2026-05-20')).toBe(false);
    });

    it('honours the minute portion of HH:MM', () => {
      // 09:30 boundary — current time is exactly 09:30:00, helper
      // requires "now >= boundary" so this fires.
      expect(shouldFireToday('09:30', null, '2026-05-20')).toBe(true);
      // 09:31 is one minute in the future; not yet.
      expect(shouldFireToday('09:31', null, '2026-05-20')).toBe(false);
    });

    it('falls back to "fire immediately" on garbage trigger strings', () => {
      // Bad pref values shouldn't paint the user into a corner — be
      // permissive so they still see the day-start review.
      expect(shouldFireToday('garbage', null, '2026-05-20')).toBe(true);
      expect(shouldFireToday('25:00', null, '2026-05-20')).toBe(true);
    });
  });
});
