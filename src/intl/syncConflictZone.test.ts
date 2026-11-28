import { describe, expect, it } from 'vitest';

import { decodeConflictValue, seriesZoneConflict } from '@aperio/shared';

/**
 * A sync conflict on a series' repeat that differs only in its zone is named
 * as the series' time zone, by city (stage 14 of
 * DESIGN-series-time-zone.md); any other conflict shows its values as they
 * are.
 */
const repeat = (fields: Record<string, unknown>) => JSON.stringify({ rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], ...fields });
const conflict = (local: string | null, remote: string | null, extra: { row_kind?: string; field?: string } = {}) => ({
  row_kind: 'event',
  field: 'recurrence',
  local_value: local,
  remote_value: remote,
  ...extra,
});

describe('decodeConflictValue', () => {
  it('reads a string as its text, a scalar as written, and nothing as a dash', () => {
    expect(decodeConflictValue(JSON.stringify('Abendrunde'))).toBe('Abendrunde');
    expect(decodeConflictValue('3')).toBe('3');
    expect(decodeConflictValue('true')).toBe('true');
    expect(decodeConflictValue('null')).toBe('—');
    expect(decodeConflictValue(null)).toBe('—');
    expect(decodeConflictValue('not json')).toBe('not json');
  });

  // Stage 14 review: the phone read every object as "[object Object]".
  it('reads an object as JSON, never as [object Object]', () => {
    const value = JSON.stringify({ rrule: 'FREQ=WEEKLY;BYDAY=MO', tzid: 'Europe/Berlin' });
    expect(decodeConflictValue(value)).toBe(value);
    expect(decodeConflictValue('[1,2]')).toBe('[1,2]');
  });
});

describe('seriesZoneConflict', () => {
  it('names the two zones by city where only the zone differs', () => {
    expect(
      seriesZoneConflict(conflict(repeat({ tzid: 'Europe/Berlin' }), repeat({ tzid: 'America/New_York' }))),
    ).toEqual({ local: 'Berlin', remote: 'New York' });
  });

  it('names a repeat without a zone as UTC', () => {
    expect(seriesZoneConflict(conflict(repeat({}), repeat({ tzid: 'Asia/Tokyo' })))).toEqual({
      local: 'UTC',
      remote: 'Tokyo',
    });
  });

  it('reads two spellings of one repeat as one: key order, null and empty parts', () => {
    const local = JSON.stringify({ tzid: 'Europe/Berlin', exceptions: [], rrule: 'FREQ=WEEKLY;BYDAY=MO' });
    const remote = JSON.stringify({ rrule: 'FREQ=WEEKLY;BYDAY=MO', tzid: 'America/New_York', anchor: null });
    expect(seriesZoneConflict(conflict(local, remote))).toEqual({ local: 'Berlin', remote: 'New York' });
  });

  it('is none where anything besides the zone differs', () => {
    expect(
      seriesZoneConflict(
        conflict(repeat({ tzid: 'Europe/Berlin' }), repeat({ tzid: 'America/New_York', rrule: 'FREQ=WEEKLY;BYDAY=TU' })),
      ),
    ).toBeNull();
    expect(
      seriesZoneConflict(
        conflict(
          repeat({ tzid: 'Europe/Berlin' }),
          repeat({ tzid: 'America/New_York', exceptions: ['2026-11-30T08:00:00Z'] }),
        ),
      ),
    ).toBeNull();
  });

  it('is none for two names of one clock', () => {
    expect(seriesZoneConflict(conflict(repeat({ tzid: 'UTC' }), repeat({ tzid: null })))).toBeNull();
    expect(
      seriesZoneConflict(conflict(repeat({ tzid: 'Asia/Calcutta' }), repeat({ tzid: 'Asia/Kolkata' }))),
    ).toBeNull();
  });

  it('is none for any other field, row or value', () => {
    const zoned = repeat({ tzid: 'Europe/Berlin' });
    const other = repeat({ tzid: 'America/New_York' });
    expect(seriesZoneConflict(conflict(zoned, other, { field: 'start' }))).toBeNull();
    expect(seriesZoneConflict(conflict(zoned, other, { row_kind: 'task' }))).toBeNull();
    expect(seriesZoneConflict(conflict(null, other))).toBeNull();
    expect(seriesZoneConflict(conflict('not json', other))).toBeNull();
  });
});
