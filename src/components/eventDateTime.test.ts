import { describe, expect, it } from 'vitest';

import {
  applyDateTimeChange,
  combine,
  dateInput,
  defaultNewEventTimes,
  nextHalfHour,
  timeInput,
  toIso,
  type EventTimes,
} from './eventDateTime';

const timed = (
  startDate: string,
  startTime: string,
  endDate: string,
  endTime: string,
): EventTimes => ({ startDate, startTime, endDate, endTime, allDay: false });

describe('nextHalfHour', () => {
  it('rounds up to the next :30 within the hour', () => {
    expect(timeInput(nextHalfHour(new Date(2026, 5, 1, 14, 23)))).toBe('14:30');
  });

  it('rolls into the next hour past :30', () => {
    expect(timeInput(nextHalfHour(new Date(2026, 5, 1, 14, 45)))).toBe('15:00');
  });

  it('advances off an exact :00 boundary', () => {
    expect(timeInput(nextHalfHour(new Date(2026, 5, 1, 14, 0)))).toBe('14:30');
  });

  it('advances off an exact :30 boundary into the next hour', () => {
    expect(timeInput(nextHalfHour(new Date(2026, 5, 1, 14, 30)))).toBe('15:00');
  });

  it('zeroes the seconds', () => {
    expect(nextHalfHour(new Date(2026, 5, 1, 14, 23, 47)).getSeconds()).toBe(0);
  });
});

describe('defaultNewEventTimes', () => {
  it('uses the next :00/:30 slot when anchored on today', () => {
    const now = new Date(2026, 5, 1, 14, 23);
    const { start, end } = defaultNewEventTimes('2026-06-01', now);
    expect(dateInput(start)).toBe('2026-06-01');
    expect(timeInput(start)).toBe('14:30');
    expect(timeInput(end)).toBe('15:30');
  });

  it('uses 09:00 when anchored on another day', () => {
    const now = new Date(2026, 5, 1, 14, 23);
    const { start, end } = defaultNewEventTimes('2026-06-05', now);
    expect(dateInput(start)).toBe('2026-06-05');
    expect(timeInput(start)).toBe('09:00');
    expect(timeInput(end)).toBe('10:00');
  });

  it('falls back to the next full hour without an anchor', () => {
    const now = new Date(2026, 5, 1, 14, 23);
    const { start, end } = defaultNewEventTimes(undefined, now);
    expect(dateInput(start)).toBe('2026-06-01');
    expect(timeInput(start)).toBe('15:00');
    expect(timeInput(end)).toBe('16:00');
  });
});

describe('applyDateTimeChange — start drags the end (duration preserved)', () => {
  it('shifts the end time when the start time moves', () => {
    const next = applyDateTimeChange(
      timed('2026-06-01', '14:00', '2026-06-01', '15:00'),
      'startTime',
      '16:00',
    );
    expect(next.startTime).toBe('16:00');
    expect(next.endDate).toBe('2026-06-01');
    expect(next.endTime).toBe('17:00');
  });

  it('carries the end across midnight when the start time pushes it there', () => {
    const next = applyDateTimeChange(
      timed('2026-06-01', '23:00', '2026-06-02', '00:00'),
      'startTime',
      '23:30',
    );
    expect(next.endDate).toBe('2026-06-02');
    expect(next.endTime).toBe('00:30');
  });

  it('shifts the end date by the same number of days as the start date', () => {
    const next = applyDateTimeChange(
      timed('2026-06-01', '10:00', '2026-06-03', '12:00'),
      'startDate',
      '2026-06-05',
    );
    expect(next.endDate).toBe('2026-06-07'); // +4 days, span preserved
    expect(next.endTime).toBe('12:00');
  });

  it('shifts an all-day span by whole days and keeps the stored time', () => {
    const next = applyDateTimeChange(
      { startDate: '2026-06-01', startTime: '09:00', endDate: '2026-06-02', endTime: '10:00', allDay: true },
      'startDate',
      '2026-06-10',
    );
    expect(next.endDate).toBe('2026-06-11'); // +9 days
    expect(next.endTime).toBe('10:00'); // untouched for all-day
  });
});

describe('applyDateTimeChange — end only resizes, never precedes the start', () => {
  it('changes only the end when the end time moves forward', () => {
    const next = applyDateTimeChange(
      timed('2026-06-01', '14:00', '2026-06-01', '15:00'),
      'endTime',
      '16:30',
    );
    expect(next.startTime).toBe('14:00');
    expect(next.endTime).toBe('16:30');
  });

  it('clamps the end to the start when an end time would precede it', () => {
    const next = applyDateTimeChange(
      timed('2026-06-01', '14:00', '2026-06-01', '15:00'),
      'endTime',
      '13:00',
    );
    expect(next.endDate).toBe('2026-06-01');
    expect(next.endTime).toBe('14:00');
  });

  it('clamps the end to the start when an end date would precede it', () => {
    const next = applyDateTimeChange(
      timed('2026-06-05', '10:00', '2026-06-05', '11:00'),
      'endDate',
      '2026-06-01',
    );
    expect(next.endDate).toBe('2026-06-05');
    expect(next.endTime).toBe('10:00');
  });
});

describe('combine / toIso round-trips', () => {
  it('combines date + time into a local Date', () => {
    const d = combine('2026-06-01', '14:30', false);
    expect(d?.getFullYear()).toBe(2026);
    expect(d?.getHours()).toBe(14);
    expect(d?.getMinutes()).toBe(30);
  });

  it('ignores the time for all-day (midnight)', () => {
    const d = combine('2026-06-01', '14:30', true);
    expect(d?.getHours()).toBe(0);
    expect(d?.getMinutes()).toBe(0);
  });

  it('returns null on a malformed date', () => {
    expect(combine('not-a-date', '14:30', false)).toBeNull();
    expect(toIso('', '14:30', false)).toBeNull();
  });
});
