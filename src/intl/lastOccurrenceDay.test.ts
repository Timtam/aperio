import { describe, expect, it } from 'vitest';

import { lastOccurrenceDayKey } from '@aperio/shared';

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

  it('reads a zoned rule on the clock the series recurs in', () => {
    // Apple writes the local end of day as UTC: 21:59:59Z is 23:59:59 in
    // Berlin on the same day, so the Monday bound belongs to that Monday and
    // not to the week before.
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
