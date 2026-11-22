import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  DEVICE_CLOCK,
  applyClockChange,
  chooseZone,
  defaultSeriesZone,
  fieldsAt,
  fieldsOnView,
  openZoneForm,
  savedRecurrence,
  unusableZoneRefusal,
  withAllDay,
  zoneFormView,
  zoneToSave,
  type ZoneFormFacts,
} from '@aperio/shared';

/**
 * The series clock form model (stage 10 of DESIGN-series-time-zone.md):
 * where the choice shows, the clock the fields stand on, the default zone,
 * what a choice does and what a save writes.
 */

const facts = (over: Partial<ZoneFormFacts> = {}): ZoneFormFacts => ({
  repeats: true,
  allDay: false,
  calendarZones: 'all',
  deviceZone: 'Europe/Berlin',
  ...over,
});

describe('where the choice shows (the table in "Wo die Auswahl erscheint")', () => {
  it('follows the rows of the table', () => {
    const row = (editing: Parameters<typeof openZoneForm>[0], over: Partial<ZoneFormFacts> = {}) =>
      zoneFormView(openZoneForm(editing, editing === 'new' || editing === 'single' ? undefined : 'Europe/Berlin', false), facts(over))
        .choice;
    expect(row('new', { repeats: false })).toBe('none');
    expect(row('new')).toBe('picker');
    expect(row('new', { allDay: true })).toBe('none');
    expect(row('single')).toBe('picker');
    expect(row('series')).toBe('picker');
    expect(row('series', { repeats: false })).toBe('none');
    expect(row('following')).toBe('hint');
    expect(row('occurrence')).toBe('none');
    expect(row('scope-in-form')).toBe('none');
    // A calendar that stores no zones shows no choice.
    expect(row('new', { calendarZones: 'none' })).toBe('none');
  });

  it('puts the fields on the series clock where it shows, on the device elsewhere', () => {
    const series = openZoneForm('series', 'America/New_York', false);
    expect(zoneFormView(series, facts()).clock).toEqual({ kind: 'series', tzid: 'America/New_York' });
    expect(zoneFormView(series, facts({ repeats: false })).clock).toEqual(DEVICE_CLOCK);
    expect(zoneFormView(series, facts({ allDay: true })).clock).toEqual(DEVICE_CLOCK);
    // A series without a zone opens on UTC.
    expect(zoneFormView(openZoneForm('series', null, false), facts())).toMatchObject({
      clock: { kind: 'series', tzid: null },
      zone: null,
    });
  });
});

describe('the zone a new series starts with', () => {
  it('is the device zone, written canonically (117a)', () => {
    expect(defaultSeriesZone('Asia/Calcutta', 'all')).toBe('Asia/Kolkata');
    expect(defaultSeriesZone('Europe/Berlin', 'all')).toBe('Europe/Berlin');
  });

  it('is UTC for a device on UTC or an unknown zone (26a), or a calendar that cannot store it (22a)', () => {
    expect(defaultSeriesZone('UTC', 'all')).toBeNull();
    expect(defaultSeriesZone('Etc/Unknown', 'all')).toBeNull();
    expect(defaultSeriesZone(null, 'all')).toBeNull();
    expect(defaultSeriesZone('America/Ciudad_Juarez', ['Europe/Berlin'])).toBeNull();
    expect(defaultSeriesZone('Europe/Berlin', ['Europe/Berlin'])).toBe('Europe/Berlin');
    expect(defaultSeriesZone('Europe/Berlin', 'none')).toBeNull();
  });
});

describe('what a save writes ("Prüfen und Speichern")', () => {
  it('writes an untouched stored zone literally', () => {
    const form = openZoneForm('series', 'Asia/Calcutta', false);
    expect(zoneToSave(form, facts())).toBe('Asia/Calcutta');
    expect(zoneToSave(openZoneForm('series', null, false), facts())).toBeNull();
  });

  it('writes a chosen zone canonically, and a chosen UTC as none', () => {
    const form = openZoneForm('series', 'Europe/Berlin', false);
    expect(zoneToSave({ ...form, chosen: 'Asia/Calcutta' }, facts())).toBe('Asia/Kolkata');
    expect(zoneToSave({ ...form, chosen: null }, facts())).toBeNull();
  });

  it('gives a new series the default, never over a chosen UTC', () => {
    const form = openZoneForm('new', undefined, false);
    expect(zoneToSave(form, facts())).toBe('Europe/Berlin');
    expect(zoneToSave({ ...form, chosen: null }, facts())).toBeNull();
    expect(zoneToSave(openZoneForm('single', undefined, false), facts())).toBe('Europe/Berlin');
  });

  it('writes no zone without a repeat, and none for a series made all-day here', () => {
    expect(zoneToSave(openZoneForm('series', 'Europe/Berlin', false), facts({ repeats: false }))).toBeNull();
    const madeAllDay = withAllDay(openZoneForm('series', 'Europe/Berlin', false), true);
    expect(zoneToSave(madeAllDay, facts({ allDay: true }))).toBeNull();
  });

  it('keeps a stored all-day series\' value, and gives it the default once it gets times', () => {
    const allDay = openZoneForm('series', null, true);
    expect(zoneToSave(allDay, facts({ allDay: true }))).toBeNull();
    expect(zoneToSave(withAllDay(allDay, false), facts())).toBe('Europe/Berlin');
    const zoned = openZoneForm('series', 'America/New_York', true);
    expect(zoneToSave(zoned, facts({ allDay: true }))).toBe('America/New_York');
    expect(zoneToSave(withAllDay(zoned, false), facts())).toBe('America/New_York');
  });

  it('builds the recurrence with the series\' exceptions and the zone', () => {
    expect(savedRecurrence('FREQ=DAILY', { exceptions: ['2026-11-03T08:00:00Z'] }, 'Europe/Berlin')).toEqual({
      rrule: 'FREQ=DAILY',
      exceptions: ['2026-11-03T08:00:00Z'],
      tzid: 'Europe/Berlin',
    });
    expect(savedRecurrence('FREQ=DAILY', null, null)).toEqual({ rrule: 'FREQ=DAILY', exceptions: [] });
    expect(savedRecurrence(null, null, 'Europe/Berlin')).toBeNull();
  });
});

describe('a zone this device cannot use', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('stands on UTC, and refuses changed times or a changed rule until another zone is chosen', () => {
    const Real = Intl.DateTimeFormat;
    vi.spyOn(Intl, 'DateTimeFormat').mockImplementation(function (
      locales?: string | string[],
      options?: Intl.DateTimeFormatOptions,
    ) {
      if (options?.timeZone === 'America/Ciudad_Juarez') throw new RangeError('no such zone');
      return new Real(locales, options);
    } as unknown as typeof Intl.DateTimeFormat);
    const form = openZoneForm('series', 'America/Ciudad_Juarez', false);
    const view = zoneFormView(form, facts());
    expect(view).toMatchObject({ clock: { kind: 'series', tzid: null }, unusable: 'America/Ciudad_Juarez' });
    expect(unusableZoneRefusal(view, form, { times: true, rule: false })).toBe('America/Ciudad_Juarez');
    expect(unusableZoneRefusal(view, form, { times: false, rule: true })).toBe('America/Ciudad_Juarez');
    expect(unusableZoneRefusal(view, form, { times: false, rule: false })).toBeNull();
    expect(unusableZoneRefusal(view, { ...form, chosen: 'Europe/Berlin' }, { times: true, rule: true })).toBeNull();
  });
});

describe('choosing a zone ("Die Zone wechseln")', () => {
  it('keeps the typed times of a new event, now on the new clock, without asking', () => {
    const form = openZoneForm('new', undefined, false);
    const fields = {
      clock: { kind: 'series' as const, tzid: 'Europe/Berlin' },
      start: { date: '2026-09-14', time: '09:00', at: null },
      end: { date: '2026-09-14', time: '10:00', at: null },
    };
    const result = chooseZone(form, fields, 'America/New_York', { rrule: 'FREQ=WEEKLY', exceptions: [] });
    expect(result.change).toBeNull();
    expect(result.form.chosen).toBe('America/New_York');
    expect(result.fields).toEqual({
      clock: { kind: 'series', tzid: 'America/New_York' },
      start: { date: '2026-09-14', time: '09:00', at: null },
      end: { date: '2026-09-14', time: '10:00', at: null },
    });
  });

  it('keeps the instant of a stored series and asks the core about its days', () => {
    // A series that lost its Berlin zone: Mondays from Sunday 23:30 UTC.
    const form = openZoneForm('series', null, false);
    const fields = fieldsAt('2026-11-01T23:30:00Z', '2026-11-02T00:30:00Z', { kind: 'series', tzid: null });
    const result = chooseZone(form, fields, 'Europe/Berlin', { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [] });
    expect(result.fields.start).toEqual({ date: '2026-11-02', time: '00:30', at: '2026-11-01T23:30:00Z' });
    expect(result.change).toMatchObject({ asks: true, fit: 'after' });
    // As the rule says, the start stays where it is: the fields do not move.
    const asRule = result.change!.asRule;
    expect(applyClockChange(result.fields, asRule)).toEqual(result.fields);
  });

  it('moves the start and the end together when the answer moves the start', () => {
    // Sundays from Sunday 23:30 UTC: as the rule says, it starts Sunday 00:30
    // Berlin, a day earlier.
    const form = openZoneForm('series', null, false);
    const fields = fieldsAt('2026-11-01T23:30:00Z', '2026-11-02T00:30:00Z', { kind: 'series', tzid: null });
    const result = chooseZone(form, fields, 'Europe/Berlin', { rrule: 'FREQ=WEEKLY;BYDAY=SU', exceptions: [] });
    expect(result.change?.asks).toBe(false);
    const moved = applyClockChange(result.fields, result.change!.asRule);
    expect(moved.start).toMatchObject({ date: '2026-11-01', time: '00:30' });
    expect(moved.end).toMatchObject({ date: '2026-11-01', time: '01:30' });
  });
});

describe('switching repeat or all-day', () => {
  it('puts the fields back on the device clock without a repeat, and on the series clock with one', () => {
    const form = openZoneForm('series', 'America/New_York', false);
    const fields = fieldsAt('2026-11-02T14:00:00Z', '2026-11-02T15:00:00Z', { kind: 'series', tzid: 'America/New_York' });
    const off = fieldsOnView(fields, zoneFormView(form, facts({ repeats: false })));
    expect(off.clock).toEqual(DEVICE_CLOCK);
    const on = fieldsOnView(off, zoneFormView(form, facts()));
    expect(on).toEqual(fields);
  });

  it('marks all-day switched on only when it was not on when the editor opened', () => {
    expect(withAllDay(openZoneForm('series', 'Europe/Berlin', false), true).allDayTurnedOn).toBe(true);
    expect(withAllDay(openZoneForm('series', 'Europe/Berlin', true), true).allDayTurnedOn).toBe(false);
    expect(withAllDay(withAllDay(openZoneForm('series', 'Europe/Berlin', false), true), false).allDayTurnedOn).toBe(false);
  });
});
