import { describe, expect, it } from 'vitest';

import { exceptionsAtSeriesTime, expandEvent, seriesTimesFromOccurrenceEdit } from './recurrence';

// A weekly series at 09:00 New York time, written in June (EDT, UTC-4). The
// zone is not the device's on any machine these run on, so the series' clock
// and the device's clock differ.
const SERIES = { start: '2026-06-15T13:00:00.000Z', end: '2026-06-15T14:00:00.000Z' };
const NY = 'America/New_York';
/** A later occurrence, still in summer time. */
const JULY = { start: '2026-07-06T13:00:00.000Z', end: '2026-07-06T14:00:00.000Z' };
/** An occurrence in winter time (EST, UTC-5): 09:00 there is 14:00 UTC. */
const DECEMBER = { start: '2026-12-07T14:00:00.000Z', end: '2026-12-07T15:00:00.000Z' };

/** `iso` moved by `days` on the device's calendar, keeping the device's reading. */
function onDevice(iso: string, days: number): string {
  const when = new Date(iso);
  when.setDate(when.getDate() + days);
  return when.toISOString();
}

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

  it('keeps the series time when only the date moved across a clock change on the device', () => {
    // A series without a zone runs in UTC. The form shows the device's clock,
    // and an untouched time a week later, past the device's clock change, is
    // another UTC time; the series must still keep its own.
    const series = { start: '2026-06-15T07:00:00.000Z', end: '2026-06-15T08:00:00.000Z' };
    const occurrence = { start: '2026-10-19T07:00:00.000Z', end: '2026-10-19T08:00:00.000Z' };
    const edited = { start: onDevice(occurrence.start, 7), end: onDevice(occurrence.end, 7) };
    expect(seriesTimesFromOccurrenceEdit(series, null, occurrence, edited, false)).toEqual({
      start: '2026-06-22T07:00:00.000Z',
      end: '2026-06-22T08:00:00.000Z',
    });
  });

  it('counts an untouched time by device days across a clock change near midnight', () => {
    // A series without a zone at 23:30 UTC. Its October occurrence moved a week
    // later on the device, time untouched: past the device's clock change that
    // is 00:30 UTC the next day, but the series still moves by seven days.
    const series = { start: '2026-06-15T23:30:00.000Z', end: '2026-06-16T00:30:00.000Z' };
    const occurrence = { start: '2026-10-19T23:30:00.000Z', end: '2026-10-20T00:30:00.000Z' };
    const edited = { start: onDevice(occurrence.start, 7), end: onDevice(occurrence.end, 7) };
    expect(seriesTimesFromOccurrenceEdit(series, null, occurrence, edited, false)).toEqual({
      start: '2026-06-22T23:30:00.000Z',
      end: '2026-06-23T00:30:00.000Z',
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

  it('moves an exception a day when the new time crosses midnight on the series clock', () => {
    // 21:00 in New York becomes 01:00 the next day there, on the same date on
    // the device. The occurrences move a day on the series' clock; an exception
    // left on its day would cancel the occurrence before the one it cancelled.
    const daily = { rrule: 'FREQ=DAILY', exceptions: ['2026-06-18T01:00:00.000Z'], tzid: NY };
    expect(
      exceptionsAtSeriesTime(daily, '2026-06-15T01:00:00.000Z', '2026-06-15T05:00:00.000Z', false),
    ).toEqual({ ...daily, exceptions: ['2026-06-18T05:00:00.000Z'] });
  });

  it('keeps exceptions on their day for a rule that names its weekdays', () => {
    // Mondays at 21:00 in New York, moved to 01:00 there: the editors write the
    // rule back as it is, so the series stays on Mondays on its clock, and the
    // exception has to stay on its Monday to cancel that occurrence.
    const weekly = {
      rrule: 'FREQ=WEEKLY;BYDAY=MO',
      exceptions: ['2026-06-23T01:00:00.000Z'],
      tzid: NY,
    };
    const start = '2026-06-16T05:00:00.000Z';
    const moved = exceptionsAtSeriesTime(weekly, '2026-06-16T01:00:00.000Z', start, false);
    const occurrences = expandEvent(
      { id: 's', start, end: '2026-06-16T06:00:00.000Z', recurrence: moved },
      { start: new Date('2026-06-01T00:00:00.000Z'), end: new Date('2026-07-10T00:00:00.000Z') },
    ).map((o) => o.start);
    expect(occurrences).not.toContain('2026-06-22T05:00:00.000Z');
    expect(occurrences).toContain('2026-06-29T05:00:00.000Z');
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
