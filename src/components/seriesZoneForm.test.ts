import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  DEVICE_CLOCK,
  DEVICE_ZONE_VIEW,
  calendarMoveMode,
  prefillMoveMode,
  storedZoneOf,
  applyClockChange,
  calendarZonesOf,
  changeClockField,
  chooseZone,
  defaultSeriesZone,
  fieldsAt,
  fieldsOnView,
  openZoneForm,
  savedRecurrence,
  settledEnd,
  unusableZoneRefusal,
  withAllDay,
  zoneFormView,
  zoneEditingFor,
  zoneFacts,
  zoneChoiceWait,
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

/** Intl refusing `zone`, as a device with old zone data does. */
function deviceRefuses(zone: string): void {
  const Real = Intl.DateTimeFormat;
  vi.spyOn(Intl, 'DateTimeFormat').mockImplementation(function (
    locales?: string | string[],
    options?: Intl.DateTimeFormatOptions,
  ) {
    if (options?.timeZone === zone) throw new RangeError(`no such zone: ${zone}`);
    return new Real(locales, options);
  } as unknown as typeof Intl.DateTimeFormat);
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('where the choice shows (the table in "Wo die Auswahl erscheint")', () => {
  it('follows the rows of the table', () => {
    const row = (editing: Parameters<typeof openZoneForm>[0], over: Partial<ZoneFormFacts> = {}) =>
      zoneFormView(
        openZoneForm(editing, editing === 'new' || editing === 'single' ? undefined : 'Europe/Berlin', false),
        facts(over),
      ).choice;
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

  it('names the device zone the calendar cannot store (22a), and nothing for UTC otherwise', () => {
    const fresh = openZoneForm('new', undefined, false);
    const list = ['Europe/Berlin', 'America/New_York'];
    expect(zoneFormView(fresh, facts({ calendarZones: list, deviceZone: 'Antarctica/Troll' }))).toMatchObject({
      zone: null,
      calendarRefuses: 'Antarctica/Troll',
    });
    expect(zoneFormView(fresh, facts({ calendarZones: list, deviceZone: 'UTC' })).calendarRefuses).toBeNull();
    expect(
      zoneFormView({ ...fresh, chosen: null }, facts({ calendarZones: list })).calendarRefuses,
    ).toBeNull();
    expect(zoneFormView(fresh, facts({ calendarZones: list })).calendarRefuses).toBeNull();
  });
});

describe('the zone a new series starts with', () => {
  it('is the device zone, written canonically (117a)', () => {
    expect(defaultSeriesZone('Asia/Calcutta', 'all')).toBe('Asia/Kolkata');
    expect(defaultSeriesZone('Europe/Berlin', 'all')).toBe('Europe/Berlin');
  });

  it('keeps the device spelling where this device cannot use tzdata\'s', () => {
    deviceRefuses('Europe/Kyiv');
    expect(defaultSeriesZone('Europe/Kiev', 'all')).toBe('Europe/Kiev');
    expect(defaultSeriesZone('Europe/Kiev', ['Europe/Kyiv'])).toBe('Europe/Kiev');
    const view = zoneFormView(openZoneForm('new', undefined, false), facts({ deviceZone: 'Europe/Kiev' }));
    expect(view).toMatchObject({ clock: { kind: 'series', tzid: 'Europe/Kiev' }, unusable: null });
  });

  it('is UTC for a device on UTC or an unknown zone (26a), or a calendar that cannot store it (22a)', () => {
    expect(defaultSeriesZone('UTC', 'all')).toBeNull();
    expect(defaultSeriesZone('Etc/Unknown', 'all')).toBeNull();
    expect(defaultSeriesZone(null, 'all')).toBeNull();
    // Not a zone another test here makes Intl refuse: the views keep the
    // formatter they built for a zone, and a later refusal would not be seen.
    expect(defaultSeriesZone('America/Chicago', ['Europe/Berlin'])).toBeNull();
    expect(defaultSeriesZone('Europe/Berlin', ['Europe/Berlin'])).toBe('Europe/Berlin');
    expect(defaultSeriesZone('Europe/Berlin', 'none')).toBeNull();
  });
});

describe('what a save writes ("Prüfen und Speichern")', () => {
  it('writes an untouched stored zone literally', () => {
    expect(zoneToSave(openZoneForm('series', 'Asia/Calcutta', false), facts())).toBe('Asia/Calcutta');
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

  it('keeps a stored all-day series\' value, and gives it the device zone once it gets times', () => {
    const allDay = openZoneForm('series', null, true);
    expect(zoneToSave(allDay, facts({ allDay: true }))).toBeNull();
    expect(zoneToSave(withAllDay(allDay, false), facts())).toBe('Europe/Berlin');
    // A zone an all-day series carried from elsewhere means nothing (46a, 48a):
    // with times it gets the device's zone, as a split writes it (tailZone).
    const zoned = openZoneForm('series', 'America/New_York', true);
    expect(zoneToSave(zoned, facts({ allDay: true }))).toBe('America/New_York');
    expect(zoneToSave(withAllDay(zoned, false), facts())).toBe('Europe/Berlin');
    expect(zoneFormView(withAllDay(zoned, false), facts()).clock).toEqual({
      kind: 'series',
      tzid: 'Europe/Berlin',
    });
  });

  it('builds the recurrence with the exceptions the form holds and the zone', () => {
    expect(savedRecurrence('FREQ=DAILY', ['2026-11-03T08:00:00Z'], 'Europe/Berlin')).toEqual({
      rrule: 'FREQ=DAILY',
      exceptions: ['2026-11-03T08:00:00Z'],
      tzid: 'Europe/Berlin',
    });
    expect(savedRecurrence('FREQ=DAILY', null, null)).toEqual({ rrule: 'FREQ=DAILY', exceptions: [] });
    expect(savedRecurrence(null, null, 'Europe/Berlin')).toBeNull();
  });
});

describe('a zone this device cannot use', () => {
  it('stands on UTC, and refuses changed times or a changed rule until another zone is chosen', () => {
    deviceRefuses('America/Ciudad_Juarez');
    const form = openZoneForm('series', 'America/Ciudad_Juarez', false);
    const view = zoneFormView(form, facts());
    expect(view).toMatchObject({ clock: { kind: 'series', tzid: null }, unusable: 'America/Ciudad_Juarez' });
    expect(unusableZoneRefusal(view, form, { times: true, rule: false })).toBe('America/Ciudad_Juarez');
    expect(unusableZoneRefusal(view, form, { times: false, rule: true })).toBe('America/Ciudad_Juarez');
    expect(unusableZoneRefusal(view, form, { times: false, rule: false })).toBeNull();
    expect(unusableZoneRefusal(view, { ...form, chosen: 'Europe/Berlin' }, { times: true, rule: true })).toBeNull();
  });

  it('asks the core about the zone the series repeats on, not the UTC its fields show here', () => {
    deviceRefuses('Europe/Kyiv');
    const form = openZoneForm('series', 'Europe/Kyiv', false);
    const view = zoneFormView(form, facts());
    // Mondays 00:30 Kyiv, shown on UTC as Sunday 22:30. Read on Kyiv the start
    // is Monday, as it is in Helsinki: no day changes and nothing is asked.
    const fields = fieldsAt('2026-11-01T22:30:00Z', '2026-11-01T23:30:00Z', view.clock);
    const result = chooseZone(form, facts(), fields, 'Europe/Helsinki', {
      rrule: 'FREQ=WEEKLY;BYDAY=MO',
      exceptions: [],
      endsByDate: false,
    });
    expect(result.change).toMatchObject({ asks: false, startDayBefore: '2026-11-02', startDayAfter: '2026-11-02' });
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
    const result = chooseZone(form, facts(), fields, 'America/New_York', { rrule: 'FREQ=WEEKLY', exceptions: [], endsByDate: false });
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
    const result = chooseZone(form, facts(), fields, 'Europe/Berlin', { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], endsByDate: false });
    expect(result.fields.start).toEqual({ date: '2026-11-02', time: '00:30', at: '2026-11-01T23:30:00Z' });
    expect(result.change).toMatchObject({ asks: true, fit: 'after' });
    // As the rule says, the start stays where it is: the fields do not move.
    const applied = applyClockChange(result.fields, result.change!.asRule);
    expect(applied).toEqual({ fields: result.fields, rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [] });
  });

  it('moves the start and the end together when the answer moves the start', () => {
    // Sundays from Sunday 23:30 UTC: as the rule says, it starts Sunday 00:30
    // Berlin, a day earlier.
    const form = openZoneForm('series', null, false);
    const fields = fieldsAt('2026-11-01T23:30:00Z', '2026-11-02T00:30:00Z', { kind: 'series', tzid: null });
    const result = chooseZone(form, facts(), fields, 'Europe/Berlin', { rrule: 'FREQ=WEEKLY;BYDAY=SU', exceptions: [], endsByDate: false });
    expect(result.change?.asks).toBe(false);
    const applied = applyClockChange(result.fields, result.change!.asRule)!;
    expect(applied.fields.start).toMatchObject({ date: '2026-11-01', time: '00:30' });
    expect(applied.fields.end).toMatchObject({ date: '2026-11-01', time: '01:30' });
  });

  it('hands on the exceptions the answer moved, for the save', () => {
    const form = openZoneForm('series', 'Europe/Berlin', false);
    const fields = fieldsAt('2026-11-02T08:00:00Z', '2026-11-02T09:00:00Z', { kind: 'series', tzid: 'Europe/Berlin' });
    const series = { rrule: 'FREQ=WEEKLY;BYDAY=MO,WE', exceptions: ['2027-03-17T08:00:00Z'], endsByDate: false };
    const result = chooseZone(form, facts(), fields, 'America/New_York', series);
    const applied = applyClockChange(result.fields, result.change!.asNow)!;
    expect(applied.exceptions).toEqual(['2027-03-17T07:00:00Z']);
    expect(savedRecurrence(applied.rrule, applied.exceptions, zoneToSave(result.form, facts()))).toEqual({
      rrule: 'FREQ=WEEKLY;BYDAY=MO,WE',
      exceptions: ['2027-03-17T07:00:00Z'],
      tzid: 'America/New_York',
    });
  });

  it('refuses a change no answer can write, and leaves the zone and the fields', () => {
    const form = openZoneForm('series', 'Europe/Berlin', false);
    const fields = fieldsAt('2026-11-02T08:00:00Z', '2026-11-02T09:00:00Z', { kind: 'series', tzid: 'Europe/Berlin' });
    const result = chooseZone(form, facts(), fields, 'America/New_York', { rrule: 'FREQ=DAILY;BYHOUR=9', exceptions: [], endsByDate: false });
    expect(result.refused).toBe('time_of_day');
    expect(result.form).toBe(form);
    expect(result.fields).toBe(fields);
  });

  it("counts a legacy end at New York's blind spot by its calendar (140a, D1)", () => {
    // Daily 19:00 New York through `20261110T235959Z`: the 105a guard reads
    // a cut before 10 November. On a calendar that stores the end as a date it
    // ends on 10 November itself, and the end moves to Chicago so.
    const form = openZoneForm('series', 'America/New_York', false);
    const fields = fieldsAt('2026-11-03T00:00:00Z', '2026-11-03T01:00:00Z', {
      kind: 'series',
      tzid: 'America/New_York',
    });
    const rule = (endsByDate: boolean) => {
      const result = chooseZone(form, facts(), fields, 'America/Chicago', {
        rrule: 'FREQ=DAILY;UNTIL=20261110T235959Z',
        exceptions: [],
        endsByDate,
      });
      const answer = result.change?.asNow;
      return answer?.outcome === 'changed' ? answer.rrule : null;
    };
    expect(rule(true)).toMatch(/UNTIL=20261111T055959Z$/);
    expect(rule(false)).toMatch(/UNTIL=20261110T055959Z$/);
  });

  it('keeps the typed end day of a single event that becomes a series (141a, D3)', () => {
    // A stored single event on 3 November, 19:00 New York, made daily here
    // through 10 November: the form holds the typed day. It is no end on any
    // clock yet, so no guard reads it; the save spells it on the new clock.
    const typedIn = (deviceZone: string, at: string, zone: string, endsByDate: boolean) => {
      const form = openZoneForm('single', undefined, false);
      const fields = fieldsAt(at, new Date(Date.parse(at) + 3_600_000).toISOString(), {
        kind: 'series',
        tzid: deviceZone,
      });
      const result = chooseZone(form, facts({ deviceZone }), fields, zone, {
        rrule: 'FREQ=DAILY;UNTIL=20261110T235959Z',
        exceptions: [],
        endsByDate,
      });
      const answers = [result.change?.asNow, result.change?.asRule];
      return answers.map((answer) => (answer?.outcome === 'changed' ? answer.rrule : null));
    };
    for (const endsByDate of [false, true]) {
      expect(typedIn('America/New_York', '2026-11-04T00:00:00Z', 'America/Chicago', endsByDate)).toEqual([
        'FREQ=DAILY;UNTIL=20261110T235959Z',
        'FREQ=DAILY;UNTIL=20261110T235959Z',
      ]);
      expect(typedIn('Europe/Berlin', '2026-11-03T00:00:00Z', 'America/New_York', endsByDate)).toEqual([
        'FREQ=DAILY;UNTIL=20261110T235959Z',
        'FREQ=DAILY;UNTIL=20261110T235959Z',
      ]);
    }
    // Spelled on the clock chosen, it reaches 10 November there.
    expect(
      settledEnd('FREQ=DAILY;UNTIL=20261110T235959Z', {
        start: '2026-11-04T00:00:00Z',
        allDay: false,
        tzid: 'America/Chicago',
      }),
    ).toBe('FREQ=DAILY;UNTIL=20261111T055959Z');
  });

  it("keeps the typed end day of a file's series, which says it holds one (`importedRule`)", () => {
    // A file's daily series at 19:00 New York, its end read as 10 November
    // when the editor was filled. Asked as a stored end, the core would read
    // that day state on New York's clock, guarded, as 9 November.
    const form = openZoneForm('series', 'America/New_York', false);
    const fields = fieldsAt('2026-11-03T00:00:00Z', '2026-11-03T01:00:00Z', {
      kind: 'series',
      tzid: 'America/New_York',
    });
    const result = chooseZone(form, facts({ calendarMoved: true }), fields, 'America/Chicago', {
      rrule: 'FREQ=DAILY;UNTIL=20261110T235959Z',
      exceptions: [],
      endsByDate: false,
      typedEnd: true,
    });
    const answers = [result.change?.asNow, result.change?.asRule];
    expect(answers.map((answer) => (answer?.outcome === 'changed' ? answer.rrule : null))).toEqual([
      'FREQ=DAILY;UNTIL=20261110T235959Z',
      'FREQ=DAILY;UNTIL=20261110T235959Z',
    ]);
  });

  it("gives a weekly single event's answers the typed end beside the days they name", () => {
    // Mondays 01:00 Berlin become Sundays 19:00 New York as they fall now.
    const form = openZoneForm('single', undefined, false);
    const fields = fieldsAt('2026-11-02T00:00:00Z', '2026-11-02T01:00:00Z', {
      kind: 'series',
      tzid: 'Europe/Berlin',
    });
    const result = chooseZone(form, facts(), fields, 'America/New_York', {
      rrule: 'FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z',
      exceptions: [],
      endsByDate: false,
    });
    const answer = result.change?.asNow;
    expect(answer?.outcome === 'changed' ? answer.rrule : null).toBe(
      'FREQ=WEEKLY;BYDAY=SU;UNTIL=20261130T235959Z',
    );
  });

  it('does not ask the core for fields carried as typed', () => {
    const form = openZoneForm('series', null, false);
    const fields = changeClockField(
      fieldsAt('2026-11-01T23:30:00Z', '2026-11-02T00:30:00Z', { kind: 'series', tzid: null }),
      'endTime',
      '',
    );
    const result = chooseZone(form, facts(), fields, 'Europe/Berlin', { rrule: 'FREQ=WEEKLY;BYDAY=SU', exceptions: [], endsByDate: false });
    expect(result.change).toBeNull();
    expect(result.fields.start).toMatchObject({ date: '2026-11-01', time: '23:30', carried: true });
  });
});

describe('switching repeat or all-day', () => {
  it('keeps the instants when repeat is switched off and on', () => {
    const form = openZoneForm('series', 'America/New_York', false);
    const fields = fieldsAt('2026-11-02T14:00:00Z', '2026-11-02T15:00:00Z', { kind: 'series', tzid: 'America/New_York' });
    const off = fieldsOnView(fields, zoneFormView(form, facts({ repeats: false })), 'instants');
    expect(off.clock).toEqual(DEVICE_CLOCK);
    const on = fieldsOnView(off, zoneFormView(form, facts()), 'instants');
    expect(on).toEqual(fields);
  });

  it('keeps the dates when all-day is switched on, and the times as typed when it is switched off', () => {
    const form = openZoneForm('series', 'America/New_York', false);
    // 2 November 21:00 New York is 3 November on every clock east of it.
    const fields = fieldsAt('2026-11-03T02:00:00Z', '2026-11-03T03:00:00Z', { kind: 'series', tzid: 'America/New_York' });
    const allDay = withAllDay(form, true);
    const on = fieldsOnView(fields, zoneFormView(allDay, facts({ allDay: true })), 'values');
    expect(on.start).toEqual({ date: '2026-11-02', time: '21:00', at: null });
    const moved = changeClockField(on, 'startDate', '2026-11-10');
    const off = fieldsOnView(moved, zoneFormView(withAllDay(allDay, false), facts()), 'values');
    expect(off.clock).toEqual({ kind: 'series', tzid: 'America/New_York' });
    expect(off.start).toEqual({ date: '2026-11-10', time: '21:00', at: null });
  });

  it('marks all-day switched on only when it was not on when the editor opened', () => {
    expect(withAllDay(openZoneForm('series', 'Europe/Berlin', false), true).allDayTurnedOn).toBe(true);
    expect(withAllDay(openZoneForm('series', 'Europe/Berlin', true), true).allDayTurnedOn).toBe(false);
    expect(withAllDay(withAllDay(openZoneForm('series', 'Europe/Berlin', false), true), false).allDayTurnedOn).toBe(false);
  });
});

describe('the row an editor stands on (zoneEditingFor)', () => {
  const ask = (over: Partial<Parameters<typeof zoneEditingFor>[0]> = {}) =>
    zoneEditingFor({
      isEdit: true,
      isOccurrence: false,
      repeats: true,
      scope: null,
      imported: false,
      readOnly: false,
      ...over,
    });

  it('follows the table for both editors', () => {
    expect(ask({ isEdit: false, repeats: false })).toBe('new');
    expect(ask()).toBe('series');
    expect(ask({ repeats: false })).toBe('single');
    expect(ask({ isOccurrence: true, scope: 'series' })).toBe('series');
    expect(ask({ isOccurrence: true, scope: 'this_and_future' })).toBe('following');
    expect(ask({ isOccurrence: true, scope: 'occurrence' })).toBe('occurrence');
    expect(ask({ isOccurrence: true, scope: null })).toBe('scope-in-form');
  });

  it('counts an appointment from a file as a stored one (136a), and leaves a read-only one on the device', () => {
    expect(ask({ isEdit: false, imported: true })).toBe('series');
    expect(ask({ isEdit: false, imported: true, repeats: false })).toBe('single');
    expect(ask({ readOnly: true })).toBeNull();
    expect(ask({ isEdit: false, imported: true, readOnly: true })).toBeNull();
  });
});

describe('the facts of a form (zoneFacts)', () => {
  it('reads the repeat, the calendar list and the device', () => {
    expect(
      zoneFacts({
        rrule: 'FREQ=WEEKLY',
        allDay: false,
        calendar: { recurrence_capabilities: { time_zones: ['Europe/Berlin'] } },
        deviceZone: 'Europe/Berlin',
      }),
    ).toEqual({
      repeats: true,
      allDay: false,
      calendarZones: ['Europe/Berlin'],
      deviceZone: 'Europe/Berlin',
      calendarMoved: false,
    });
    const blank = zoneFacts({ rrule: ' ', allDay: true, calendar: undefined, deviceZone: null, calendarMoved: true });
    expect(blank).toMatchObject({ repeats: false, calendarZones: 'all', calendarMoved: true });
    expect(calendarZonesOf({ recurrence_capabilities: null })).toBe('all');
  });
});

describe('a zone the target calendar cannot store (139a)', () => {
  const exchange = ['Europe/Berlin', 'Asia/Kolkata'];

  it('is named where the event moves to that calendar, canonically', () => {
    const form = openZoneForm('series', 'America/New_York', false);
    expect(zoneFormView(form, facts({ calendarZones: exchange, calendarMoved: true })).notStorable).toBe(
      'America/New_York',
    );
    expect(zoneFormView(openZoneForm('series', 'Asia/Calcutta', false), facts({ calendarZones: exchange, calendarMoved: true })).notStorable).toBeNull();
  });

  it('is not named for an untouched zone in its own calendar, nor for UTC', () => {
    const form = openZoneForm('series', 'America/New_York', false);
    expect(zoneFormView(form, facts({ calendarZones: exchange })).notStorable).toBeNull();
    const utc = openZoneForm('series', null, false);
    expect(zoneFormView(utc, facts({ calendarZones: exchange, calendarMoved: true })).notStorable).toBeNull();
  });

  it('is not named for UTC under another name, nor for a name tzdata does not know', () => {
    for (const stored of ['UTC', 'Etc/UTC', 'GMT', 'Pacific Standard Time']) {
      const form = openZoneForm('series', stored, false);
      expect(zoneFormView(form, facts({ calendarZones: exchange, calendarMoved: true })).notStorable, stored).toBeNull();
    }
  });

  it('is named for a chosen zone, which stays', () => {
    const chosen = { ...openZoneForm('new', undefined, false), chosen: 'America/New_York' };
    const view = zoneFormView(chosen, facts({ calendarZones: exchange }));
    expect(view.zone).toBe('America/New_York');
    expect(view.notStorable).toBe('America/New_York');
  });
});

describe('a typed end on a zone choice', () => {
  it('lets the core move the end of a rule that repeats more than once a day, as a stored one', () => {
    const series = {
      rrule: 'FREQ=HOURLY;INTERVAL=6;UNTIL=20261028T143000Z',
      exceptions: [],
      endsByDate: false,
    };
    const NY = { kind: 'series', tzid: 'America/New_York' } as const;
    const fields = fieldsAt('2026-10-01T14:00:00Z', '2026-10-01T15:00:00Z', NY);
    const form = openZoneForm('series', 'America/New_York', false);
    const typed = chooseZone(form, facts(), fields, 'Europe/Berlin', { ...series, typedEnd: true });
    const stored = chooseZone(form, facts(), fields, 'Europe/Berlin', series);
    expect(typed.change).toEqual(stored.change);
    expect(stored.change?.asNow).toMatchObject({ outcome: 'changed', rrule: 'FREQ=HOURLY;INTERVAL=6;UNTIL=20261028T153000Z' });
  });
});

describe('a calendar change (question 8 of stage 12)', () => {
  it('keeps the instant on the device clock, the typed times of an untouched new series, and the instants otherwise', () => {
    const series = zoneFormView(openZoneForm('series', 'Europe/Berlin', false), facts());
    const device = zoneFormView(openZoneForm('new', undefined, false), facts({ calendarZones: 'none' }));
    expect(device).toBe(DEVICE_ZONE_VIEW);
    expect(calendarMoveMode(openZoneForm('new', undefined, false), device)).toBe('instants');
    expect(calendarMoveMode(openZoneForm('new', undefined, false), series)).toBe('values');
    expect(calendarMoveMode({ ...openZoneForm('new', undefined, false), chosen: null }, series)).toBe('instants');
    expect(calendarMoveMode(openZoneForm('series', 'Europe/Berlin', false), series)).toBe('instants');
  });

  it('reads the stored zone of what opened', () => {
    expect(storedZoneOf({ tzid: 'Europe/Kiev' })).toBe('Europe/Kiev');
    expect(storedZoneOf({ tzid: null })).toBeNull();
    expect(storedZoneOf({})).toBeNull();
    expect(storedZoneOf(null)).toBeUndefined();
  });
});

describe('a title suggestion (prefillMoveMode)', () => {
  const untouchedNew = openZoneForm('new', undefined, false);
  const onBerlin = zoneFormView(untouchedNew, facts());
  const onUtc = zoneFormView(untouchedNew, facts({ calendarZones: ['America/New_York'] }));

  const repeating = { allDay: false, repeatedBefore: true };

  it('keeps the dates where it changes all-day', () => {
    expect(prefillMoveMode(untouchedNew, onBerlin, { allDay: true, repeatedBefore: false })).toBe('values');
  });

  it('keeps the instant where it turns the repeat on, as the repeat field does', () => {
    expect(prefillMoveMode(untouchedNew, onUtc, { allDay: false, repeatedBefore: false })).toBe('instants');
  });

  it('moves as its calendar would where the form already repeats, from a calendar without zones too', () => {
    expect(onUtc.clock).toEqual({ kind: 'series', tzid: null });
    expect(prefillMoveMode(untouchedNew, onUtc, repeating)).toBe(calendarMoveMode(untouchedNew, onUtc));
    expect(prefillMoveMode(untouchedNew, onUtc, repeating)).toBe('values');
    // A repeating new event in a device calendar stands on the device's clock
    // (DEVICE_ZONE_VIEW) and still repeats.
    expect(zoneFormView(untouchedNew, facts({ calendarZones: 'none' }))).toBe(DEVICE_ZONE_VIEW);
    const series = openZoneForm('series', 'Europe/Berlin', false);
    expect(prefillMoveMode(series, onUtc, repeating)).toBe('instants');
  });
});

describe('when a zone choice waits (zoneChoiceWait)', () => {
  const NY = { kind: 'series', tzid: 'America/New_York' } as const;
  const stored = '2026-09-14T13:00:00.000Z';
  const fields = fieldsAt(stored, '2026-09-14T14:00:00.000Z', NY);
  const series = openZoneForm('series', 'America/New_York', false);

  it('waits for a save where the start of a stored series moved', () => {
    expect(zoneChoiceWait(series, fields, stored)).toBeNull();
    expect(zoneChoiceWait(series, changeClockField(fields, 'startDate', '2026-09-21'), stored)).toBe('start');
    expect(zoneChoiceWait(series, changeClockField(fields, 'startTime', '10:00'), stored)).toBe('start');
    // Only the end changed: the start is the one the series opened with.
    expect(zoneChoiceWait(series, changeClockField(fields, 'endTime', '11:00'), stored)).toBeNull();
  });

  it('waits for a save where a series that opened all-day got times', () => {
    expect(zoneChoiceWait(openZoneForm('series', null, true), fields, stored)).toBe('start');
  });

  it('waits for valid times where the start or the end names no instant', () => {
    // 14 March 2027, 02:30 does not exist in New York.
    const gapEnd = changeClockField(changeClockField(fields, 'endDate', '2027-03-14'), 'endTime', '02:30');
    expect(zoneChoiceWait(series, gapEnd, stored)).toBe('times');
    expect(zoneChoiceWait(series, changeClockField(fields, 'startTime', ''), stored)).toBe('times');
  });

  it('never waits for a new event, a single event or an import', () => {
    const moved = changeClockField(fields, 'startDate', '2026-09-21');
    expect(zoneChoiceWait(openZoneForm('new', undefined, false), moved, null)).toBeNull();
    expect(zoneChoiceWait(openZoneForm('single', undefined, false), moved, stored)).toBeNull();
    expect(zoneChoiceWait(series, moved, null)).toBeNull();
  });
});
