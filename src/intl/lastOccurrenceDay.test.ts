import { describe, expect, it, vi } from 'vitest';

import { expandEvent, lastOccurrenceDayKey } from '@aperio/shared';

/**
 * Decision 85a: the day a bounded series really ends on, for the sentence
 * "letzter Termin am …".
 *
 * `UNTIL` is a BOUND, not an occurrence, and three providers write it three
 * ways — Aperio and Exchange as the end of the day in UTC, Apple as the local
 * end of day expressed in UTC, a truncation as one second before the cut. So
 * the day is asked of the same expander that fills the calendar, and the
 * sentence agrees with what the user sees there.
 */

const event = (
  rrule: string,
  start = '2026-06-15T07:00:00.000Z',
  tzid: string | null = null,
) => ({ id: 'ev-1', start, end: start, recurrence: { rrule, exceptions: [], tzid } });

describe('lastOccurrenceDayKey', () => {
  it('names the last occurrence, not the bound', () => {
    // 31 March 2027 is a Wednesday; the last Monday before it is the 29th.
    expect(
      lastOccurrenceDayKey(event('FREQ=WEEKLY;BYDAY=MO;UNTIL=20270331T235959Z')),
    ).toBe('2027-03-29');
  });

  it('names the day on the clock the series recurs in', () => {
    // A morning series in Berlin: the day is read on the series' clock, not
    // on UTC, so a bound written as the local end of day names that Monday.
    expect(
      lastOccurrenceDayKey(
        event(
          'FREQ=WEEKLY;BYDAY=MO;UNTIL=20270329T215959Z',
          '2026-06-15T07:00:00.000Z',
          'Europe/Berlin',
        ),
      ),
    ).toBe('2027-03-29');
  });

  it('answers with the occurrence the calendar shows, an evening one included', () => {
    // An evening series in Berlin: 22:00 there on Monday 29 March is 20:00
    // UTC, within the bound. The views shift the bound into the series' wall
    // time before they compare, and the sentence reads the same rule; read
    // against the bound's UTC digits, it named the Monday before.
    const series = event(
      'FREQ=WEEKLY;BYDAY=MO;UNTIL=20270329T215959Z',
      '2026-06-15T20:00:00.000Z',
      'Europe/Berlin',
    );
    expect(lastOccurrenceDayKey(series)).toBe('2027-03-29');
    const shown = expandEvent(series, {
      start: new Date('2027-03-01T00:00:00Z'),
      end: new Date('2027-04-30T00:00:00Z'),
    }).map((o) => o.start);
    expect(shown.at(-1)).toBe('2027-03-29T20:00:00.000Z');
  });

  it('names the day a series of days ends on, however its bound is spelled (201)', () => {
    // A series of New York days, read on a device in Berlin: each begins at
    // 06:00 there. The date bound covers the whole of 6 May, as the views
    // show it; read as its midnight, it named the 5th.
    const real = new Intl.DateTimeFormat().resolvedOptions();
    const spy = vi
      .spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions')
      .mockReturnValue({ ...real, timeZone: 'Europe/Berlin' });
    try {
      for (const rrule of [
        'FREQ=DAILY;UNTIL=20260506',
        'FREQ=DAILY;UNTIL=20260506T235959Z',
        'FREQ=DAILY;UNTIL=20260506T120000',
      ]) {
        const series = {
          ...event(rrule, '2026-05-04T04:00:00.000Z'),
          end: '2026-05-05T04:00:00.000Z',
          all_day: true,
        };
        expect(lastOccurrenceDayKey(series), rrule).toBe('2026-05-06');
        const shown = expandEvent(series, {
          start: new Date('2026-04-01T00:00:00Z'),
          end: new Date('2026-06-01T00:00:00Z'),
        }).map((o) => o.start);
        expect(shown.at(-1), rrule).toBe('2026-05-06T04:00:00.000Z');
      }
    } finally {
      spy.mockRestore();
    }
  });

  it('says nothing where a day would be a guess', () => {
    // A rule that ends by counting says how often instead.
    expect(lastOccurrenceDayKey(event('FREQ=WEEKLY;BYDAY=MO;COUNT=5'))).toBeNull();
    // An unbounded rule has no last day.
    expect(lastOccurrenceDayKey(event('FREQ=WEEKLY;BYDAY=MO'))).toBeNull();
    // A sub-daily bound would iterate by the minute.
    expect(lastOccurrenceDayKey(event('FREQ=MINUTELY;UNTIL=20270331T235959Z'))).toBeNull();
    // A bound before the first occurrence names nothing.
    expect(
      lastOccurrenceDayKey(event('FREQ=WEEKLY;BYDAY=MO;UNTIL=20260601T000000Z')),
    ).toBeNull();
    // And an event that does not repeat at all.
    expect(
      lastOccurrenceDayKey({
        id: 'ev-2',
        start: '2026-06-15T07:00:00.000Z',
        end: '2026-06-15T08:00:00.000Z',
        recurrence: null,
      }),
    ).toBeNull();
  });
});
