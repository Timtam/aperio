import { describe, expect, it } from 'vitest';

import { exceptionsAtSeriesTime, seriesTimesFromOccurrenceEdit } from './recurrence';

// A weekly series at 09:00 New York time, written in June (EDT, UTC-4). The
// zone is not the device's on any machine these run on, so the series' clock
// and the device's clock differ.
const SERIES = { start: '2026-06-15T13:00:00.000Z', end: '2026-06-15T14:00:00.000Z' };
const NY = 'America/New_York';
/** A later occurrence, still in summer time. */
const JULY = { start: '2026-07-06T13:00:00.000Z', end: '2026-07-06T14:00:00.000Z' };
/** An occurrence in winter time (EST, UTC-5): 09:00 there is 14:00 UTC. */
const DECEMBER = { start: '2026-12-07T14:00:00.000Z', end: '2026-12-07T15:00:00.000Z' };

describe('seriesTimesFromOccurrenceEdit', () => {
  it('leaves the series where it is when the fields are untouched', () => {
    expect(seriesTimesFromOccurrenceEdit(SERIES, NY, JULY, JULY, false)).toEqual(SERIES);
  });

  it('gives the series a new time of day on its own day', () => {
    const edited = { start: '2026-07-06T14:30:00.000Z', end: '2026-07-06T15:30:00.000Z' };
    expect(seriesTimesFromOccurrenceEdit(SERIES, NY, JULY, edited, false)).toEqual({
      start: '2026-06-15T14:30:00.000Z',
      end: '2026-06-15T15:30:00.000Z',
    });
  });

  it('moves the series start by the days the date moved', () => {
    const edited = { start: '2026-07-07T13:00:00.000Z', end: '2026-07-07T14:00:00.000Z' };
    expect(seriesTimesFromOccurrenceEdit(SERIES, NY, JULY, edited, false)).toEqual({
      start: '2026-06-16T13:00:00.000Z',
      end: '2026-06-16T14:00:00.000Z',
    });
  });

  it('reads the time of day on the series clock, across a clock change', () => {
    // Only the end of a winter occurrence changed. Its 14:00 UTC is the same
    // 09:00 in New York as the summer start, so the start must not move.
    const edited = { start: DECEMBER.start, end: '2026-12-07T15:30:00.000Z' };
    expect(seriesTimesFromOccurrenceEdit(SERIES, NY, DECEMBER, edited, false)).toEqual({
      start: SERIES.start,
      end: '2026-06-15T14:30:00.000Z',
    });
  });

  it('moves an all-day series by local days and keeps the edited length', () => {
    const local = (y: number, m: number, d: number) => new Date(y, m - 1, d).toISOString();
    const series = { start: local(2026, 6, 15), end: local(2026, 6, 16) };
    const occurrence = { start: local(2026, 7, 6), end: local(2026, 7, 7) };
    const edited = { start: local(2026, 7, 8), end: local(2026, 7, 10) };
    expect(seriesTimesFromOccurrenceEdit(series, null, occurrence, edited, true)).toEqual({
      start: local(2026, 6, 17),
      end: local(2026, 6, 19),
    });
  });
});

describe('exceptionsAtSeriesTime', () => {
  const recurrence = {
    rrule: 'FREQ=WEEKLY;BYDAY=MO',
    exceptions: ['2026-06-22T13:00:00.000Z', '2026-12-14T14:00:00.000Z'],
    tzid: NY,
  };

  it('moves every exception to the new time of day on the series clock', () => {
    expect(
      exceptionsAtSeriesTime(recurrence, SERIES.start, '2026-06-15T14:30:00.000Z', false),
    ).toEqual({
      ...recurrence,
      // 10:30 in New York: 14:30 UTC in summer, 15:30 UTC in winter.
      exceptions: ['2026-06-22T14:30:00.000Z', '2026-12-14T15:30:00.000Z'],
    });
  });

  it('leaves the exceptions alone when only the date moved', () => {
    expect(exceptionsAtSeriesTime(recurrence, SERIES.start, '2026-06-16T13:00:00.000Z', false)).toBe(
      recurrence,
    );
  });

  it('leaves an all-day series alone', () => {
    expect(exceptionsAtSeriesTime(recurrence, SERIES.start, '2026-06-15T14:30:00.000Z', true)).toBe(
      recurrence,
    );
  });
});
