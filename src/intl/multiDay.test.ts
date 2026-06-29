import { describe, expect, it } from 'vitest';

import type { CalendarEvent } from '../api/types';
import {
  buildAllDayBars,
  daysCoveredKeys,
  eventCoversDay,
  eventDayTimes,
  multiDayInfo,
} from './multiDay';

function weekStartingMonday(year: number, month: number, day: number): Date[] {
  const out: Date[] = [];
  for (let i = 0; i < 7; i++) {
    out.push(new Date(year, month, day + i));
  }
  return out;
}

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

  // Timed events use LOCAL-time strings (no Z) so the cross-midnight assertions
  // are timezone-robust — daysCoveredKeys reads the local calendar day.
  it('spreads a timed event across midnight onto both days', () => {
    const ev = mkEvent({
      start: '2026-05-20T22:00:00',
      end: '2026-05-21T02:00:00',
      all_day: false,
    });
    expect(daysCoveredKeys(ev)).toEqual(['2026-05-20', '2026-05-21']);
  });

  it('keeps a same-day timed event on one day', () => {
    const ev = mkEvent({
      start: '2026-05-20T09:00:00',
      end: '2026-05-20T10:00:00',
      all_day: false,
    });
    expect(daysCoveredKeys(ev)).toEqual(['2026-05-20']);
  });

  it('does not leak a timed event ending exactly at midnight into the next day', () => {
    const ev = mkEvent({
      start: '2026-05-20T22:00:00',
      end: '2026-05-21T00:00:00',
      all_day: false,
    });
    expect(daysCoveredKeys(ev)).toEqual(['2026-05-20']);
  });

  it('keeps a bad/NaN-end timed event on the start day only', () => {
    const ev = mkEvent({
      start: '2026-05-20T22:00:00',
      end: 'not-a-date',
      all_day: false,
    });
    expect(daysCoveredKeys(ev)).toEqual(['2026-05-20']);
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

  it('returns 1/2 and 2/2 for a timed event that crosses midnight', () => {
    const timed = mkEvent({
      start: '2026-05-20T22:00:00',
      end: '2026-05-21T02:00:00',
      all_day: false,
    });
    expect(multiDayInfo(timed, new Date(2026, 4, 20))).toEqual({
      dayIndex: 1,
      totalDays: 2,
    });
    expect(multiDayInfo(timed, new Date(2026, 4, 21))).toEqual({
      dayIndex: 2,
      totalDays: 2,
    });
  });

  it('returns null for a same-day timed event', () => {
    const timed = mkEvent({
      start: '2026-05-20T09:00:00',
      end: '2026-05-20T10:00:00',
      all_day: false,
    });
    expect(multiDayInfo(timed, new Date(2026, 4, 20))).toBeNull();
  });
});

describe('eventDayTimes', () => {
  // A deterministic 24h `HH:mm` formatter so the per-day clamp assertions are
  // timezone- and locale-robust (the real `useDateFormat` 'p' is locale-bound).
  const fmt = {
    format: (date: Date | number, _pattern: string) => {
      const d = new Date(date);
      const hh = String(d.getHours()).padStart(2, '0');
      const mm = String(d.getMinutes()).padStart(2, '0');
      return `${hh}:${mm}`;
    },
  };

  // A timed 23:00 → 01:00 meeting (local-time strings → timezone-robust).
  const crossMidnight = { start: '2026-05-20T23:00:00', end: '2026-05-21T01:00:00' };

  it('clamps the START day to "…–24:00" (end of day, NOT next 00:00)', () => {
    expect(eventDayTimes(fmt, crossMidnight, new Date(2026, 4, 20))).toEqual({
      startStr: '23:00',
      endStr: '24:00',
    });
  });

  it('clamps the TAIL day to "00:00–…" (not the absolute 23:00 start)', () => {
    expect(eventDayTimes(fmt, crossMidnight, new Date(2026, 4, 21))).toEqual({
      startStr: '00:00',
      endStr: '01:00',
    });
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

describe('buildAllDayBars', () => {
  // Mon 2026-05-18 .. Sun 2026-05-24.
  const week = weekStartingMonday(2026, 4, 18);

  it('returns no bars when no all-day events overlap the window', () => {
    const timed = mkEvent({
      all_day: false,
      start: '2026-05-20T10:00:00Z',
      end: '2026-05-20T11:00:00Z',
    });
    expect(buildAllDayBars([timed], week)).toEqual([]);
  });

  it('spans a single-day all-day event over one column', () => {
    const ev = mkEvent({
      start: '2026-05-20T00:00:00Z',
      end: '2026-05-21T00:00:00Z',
    });
    const bars = buildAllDayBars([ev], week);
    expect(bars).toHaveLength(1);
    expect(bars[0].startCol).toBe(3); // Wed
    expect(bars[0].endCol).toBe(3);
    expect(bars[0].lane).toBe(0);
    expect(bars[0].continuesBefore).toBe(false);
    expect(bars[0].continuesAfter).toBe(false);
  });

  it('clips a vacation that runs past both window edges', () => {
    // 2026-05-18 sits in column 1 (Mon); the vacation began before the
    // visible week and runs into the next one.
    const ev = mkEvent({
      start: '2026-05-15T00:00:00Z',
      end: '2026-05-30T00:00:00Z',
    });
    const bars = buildAllDayBars([ev], week);
    expect(bars).toHaveLength(1);
    expect(bars[0].startCol).toBe(1);
    expect(bars[0].endCol).toBe(7);
    expect(bars[0].continuesBefore).toBe(true);
    expect(bars[0].continuesAfter).toBe(true);
  });

  it('packs three overlapping all-day events into three lanes', () => {
    const a = mkEvent({
      id: 'a',
      start: '2026-05-18T00:00:00Z',
      end: '2026-05-21T00:00:00Z', // Mon–Wed
    });
    const b = mkEvent({
      id: 'b',
      start: '2026-05-19T00:00:00Z',
      end: '2026-05-22T00:00:00Z', // Tue–Thu
    });
    const c = mkEvent({
      id: 'c',
      start: '2026-05-20T00:00:00Z',
      end: '2026-05-23T00:00:00Z', // Wed–Fri
    });
    const bars = buildAllDayBars([a, b, c], week);
    expect(bars.map((x) => x.lane).sort()).toEqual([0, 1, 2]);
    // Leftmost (a) gets the bottom lane after the sort.
    const aBar = bars.find((x) => x.event.id === 'a')!;
    expect(aBar.lane).toBe(0);
  });

  it('reuses a lane when the next bar starts after the previous ends', () => {
    const earlyWeek = mkEvent({
      id: 'early',
      start: '2026-05-18T00:00:00Z',
      end: '2026-05-20T00:00:00Z', // Mon–Tue
    });
    const lateWeek = mkEvent({
      id: 'late',
      start: '2026-05-21T00:00:00Z',
      end: '2026-05-23T00:00:00Z', // Thu–Fri
    });
    const bars = buildAllDayBars([earlyWeek, lateWeek], week);
    // Both can share lane 0 — they don't overlap.
    expect(bars.every((b) => b.lane === 0)).toBe(true);
  });

  it('returns an empty array for an empty days window', () => {
    expect(buildAllDayBars([mkEvent({})], [])).toEqual([]);
  });
});
