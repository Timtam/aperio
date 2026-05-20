import { describe, expect, it } from 'vitest';

import type { CalendarEvent } from '../api/types';
import {
  daysCoveredKeys,
  eventCoversDay,
  multiDayInfo,
} from './multiDay';

function mkEvent(over: Partial<CalendarEvent>): CalendarEvent {
  return {
    id: 'e1',
    calendar_id: 'cal',
    title: 'Urlaub',
    description: null,
    location: null,
    start: '2026-05-20T00:00:00Z',
    end: '2026-06-03T00:00:00Z',
    all_day: true,
    recurrence: null,
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
    created_at: '2026-04-01T10:00:00Z',
    updated_at: '2026-04-01T10:00:00Z',
    etag: null,
    ...over,
  };
}

describe('daysCoveredKeys', () => {
  it('returns one key per day for a 14-day vacation', () => {
    const ev = mkEvent({});
    const keys = daysCoveredKeys(ev);
    // 14 days: May 20 .. June 2 inclusive (DTEND is exclusive).
    expect(keys).toHaveLength(14);
    expect(keys[0]).toBe('2026-05-20');
    expect(keys[13]).toBe('2026-06-02');
  });

  it('falls back to a single key when DTEND equals DTSTART', () => {
    const ev = mkEvent({
      start: '2026-05-20T00:00:00Z',
      end: '2026-05-20T00:00:00Z',
    });
    expect(daysCoveredKeys(ev)).toEqual(['2026-05-20']);
  });

  it('emits one key for a single-day all-day event', () => {
    const ev = mkEvent({
      start: '2026-05-20T00:00:00Z',
      end: '2026-05-21T00:00:00Z',
    });
    expect(daysCoveredKeys(ev)).toEqual(['2026-05-20']);
  });

  it('ignores end for non-all-day events (no spread on cross-midnight)', () => {
    const ev = mkEvent({
      start: '2026-05-20T22:00:00Z',
      end: '2026-05-21T02:00:00Z',
      all_day: false,
    });
    // Only the start day, regardless of crossing midnight in UTC.
    expect(daysCoveredKeys(ev)).toHaveLength(1);
  });
});

describe('multiDayInfo', () => {
  const ev = mkEvent({});

  it('returns null for a single-day all-day event', () => {
    const single = mkEvent({
      start: '2026-05-20T00:00:00Z',
      end: '2026-05-21T00:00:00Z',
    });
    expect(multiDayInfo(single, new Date(2026, 4, 20))).toBeNull();
  });

  it('returns the 1-based index and the total span', () => {
    expect(multiDayInfo(ev, new Date(2026, 4, 20))).toEqual({
      dayIndex: 1,
      totalDays: 14,
    });
    expect(multiDayInfo(ev, new Date(2026, 4, 22))).toEqual({
      dayIndex: 3,
      totalDays: 14,
    });
    expect(multiDayInfo(ev, new Date(2026, 5, 2))).toEqual({
      dayIndex: 14,
      totalDays: 14,
    });
  });

  it('returns null for a day outside the span', () => {
    expect(multiDayInfo(ev, new Date(2026, 5, 3))).toBeNull();
    expect(multiDayInfo(ev, new Date(2026, 4, 19))).toBeNull();
  });

  it('returns null for non-all-day events even if they cross midnight', () => {
    const timed = mkEvent({
      start: '2026-05-20T22:00:00Z',
      end: '2026-05-21T02:00:00Z',
      all_day: false,
    });
    expect(multiDayInfo(timed, new Date(2026, 4, 20))).toBeNull();
    expect(multiDayInfo(timed, new Date(2026, 4, 21))).toBeNull();
  });
});

describe('eventCoversDay', () => {
  it('matches every day in a multi-day all-day span', () => {
    const ev = mkEvent({});
    expect(eventCoversDay(ev, new Date(2026, 4, 20))).toBe(true);
    expect(eventCoversDay(ev, new Date(2026, 4, 25))).toBe(true);
    expect(eventCoversDay(ev, new Date(2026, 5, 2))).toBe(true);
    expect(eventCoversDay(ev, new Date(2026, 5, 3))).toBe(false);
  });
});
