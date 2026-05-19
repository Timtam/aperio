import { describe, expect, it } from 'vitest';
import {
  expandEvent,
  expandAll,
  isExpandedOccurrence,
} from './recurrence';
import type { CalendarEvent } from '../api/types';

function mkEvent(overrides: Partial<CalendarEvent> = {}): CalendarEvent {
  const start = '2026-05-19T09:00:00.000Z';
  const end = '2026-05-19T10:00:00.000Z';
  return {
    id: 'evt-1',
    calendar_id: 'cal-1',
    title: 'Standup',
    description: null,
    location: null,
    start,
    end,
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
    created_at: start,
    updated_at: start,
    etag: null,
    ...overrides,
  };
}

const ONE_DAY = 24 * 60 * 60 * 1000;

describe('expandEvent', () => {
  it('returns the event unchanged when there is no recurrence', () => {
    const ev = mkEvent();
    const out = expandEvent(ev, {
      start: new Date('2026-05-01'),
      end: new Date('2026-06-01'),
    });
    expect(out).toEqual([ev]);
  });

  it('expands a weekly rule into the right number of occurrences', () => {
    const ev = mkEvent({
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=TU',
        exceptions: [],
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-01'),
      end: new Date('2026-06-01'),
    });
    // dtstart is Tuesday 2026-05-19. Tuesdays in [dtstart, range end]
    // that fall in range: 19 and 26 May (2 June is past the range end).
    expect(out.length).toBe(2);
    out.forEach((occ) => expect(new Date(occ.start).getUTCDay()).toBe(2));
  });

  it('honours COUNT', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.length).toBe(3);
  });

  it('honours UNTIL', () => {
    const ev = mkEvent({
      recurrence: {
        rrule: 'FREQ=DAILY;UNTIL=20260521T235959Z',
        exceptions: [],
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    // 19, 20, 21 — three occurrences.
    expect(out.length).toBe(3);
  });

  it('skips EXDATE entries', () => {
    const ev = mkEvent({
      recurrence: {
        rrule: 'FREQ=DAILY;COUNT=5',
        exceptions: ['2026-05-20T09:00:00.000Z'],
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.length).toBe(4);
    expect(out.find((o) => o.start.startsWith('2026-05-20'))).toBeUndefined();
  });

  it('synthesises unique IDs and a series_id', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'FREQ=DAILY;COUNT=2', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out[0].id).not.toBe(out[1].id);
    expect(isExpandedOccurrence(out[0])).toBe(true);
    if (isExpandedOccurrence(out[0])) {
      expect(out[0].series_id).toBe('evt-1');
    }
  });

  it('preserves event duration on every occurrence', () => {
    const ev = mkEvent({
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T10:30:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    out.forEach((occ) => {
      const dur = new Date(occ.end).getTime() - new Date(occ.start).getTime();
      expect(dur).toBe(90 * 60 * 1000);
    });
  });

  it('falls back to the master event if the rule is invalid', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'BOGUS=NOPE', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-01'),
      end: new Date('2026-06-01'),
    });
    expect(out).toEqual([ev]);
  });

  it('accepts a rule with an explicit RRULE: prefix', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'RRULE:FREQ=DAILY;COUNT=2', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.length).toBe(2);
  });
});

describe('expandAll', () => {
  it('mixes single and recurring events, sorted chronologically', () => {
    const single = mkEvent({
      id: 'a',
      start: '2026-05-22T08:00:00.000Z',
      end: '2026-05-22T09:00:00.000Z',
    });
    const recurring = mkEvent({
      id: 'b',
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T10:00:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=4', exceptions: [] },
    });
    const out = expandAll([single, recurring], {
      start: new Date('2026-05-19'),
      end: new Date(Date.parse('2026-05-19') + 10 * ONE_DAY),
    });
    expect(out.length).toBe(5); // 4 daily + 1 single
    const starts = out.map((e) => e.start);
    const sorted = [...starts].sort();
    expect(starts).toEqual(sorted);
  });
});
