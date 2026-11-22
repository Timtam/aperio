import { describe, expect, it } from 'vitest';

import {
  DEVICE_CLOCK,
  changeClockField,
  clockFieldGaps,
  clockFieldLabel,
  deviceHint,
  fieldClockName,
  fieldsAt,
  fieldsOnView,
  followingHint,
  keepingInstants,
  openZoneForm,
  repeatClockAnnouncement,
  zoneCityName,
  zoneFormView,
  zoneSaveRefusal,
  zoneSentences,
  type ClockFields,
  type ZoneFormView,
} from '@aperio/shared';
import i18n from '../i18n';

/**
 * What the editors say about a series' clock (stage 12 of
 * DESIGN-series-time-zone.md): the labels that name the clock, the hint and
 * the sentences after the end time, the gap message, what the repeat field
 * says once it switched the clock, and why a save is refused. In German, and
 * on a device in Berlin whatever zone the test machine is in.
 */

const de = i18n.getFixedT('de');
const BERLIN = 'Europe/Berlin';
const NEW_YORK = { kind: 'series', tzid: 'America/New_York' } as const;

/** Monday 14 September 2026, 09:00 to 10:00 in New York: 15:00 to 16:00 in Berlin. */
const nineInNewYork = (): ClockFields =>
  fieldsAt('2026-09-14T13:00:00.000Z', '2026-09-14T14:00:00.000Z', NEW_YORK);

const view = (over: Partial<ZoneFormView>): ZoneFormView => ({
  choice: 'picker',
  clock: NEW_YORK,
  zone: 'America/New_York',
  unusable: null,
  calendarRefuses: null,
  notStorable: null,
  ...over,
});

describe('the name of a clock', () => {
  it('is the city, UTC, or the name of a zone outside the list', () => {
    expect(zoneCityName('America/New_York')).toBe('New York');
    expect(zoneCityName('Europe/Kiev')).toBe('Kyiv');
    expect(zoneCityName(null)).toBe('UTC');
    expect(zoneCityName('Etc/UTC')).toBe('UTC');
    expect(zoneCityName('EST5EDT')).toBe('EST5EDT');
    // The core reads a name tzdata does not know as UTC.
    expect(zoneCityName('Pacific Standard Time')).toBe('UTC');
  });

  it('is left out of the labels where the clock reads every instant as the device does', () => {
    expect(fieldClockName(DEVICE_CLOCK, BERLIN)).toBeNull();
    expect(fieldClockName({ kind: 'series', tzid: 'Asia/Kolkata' }, 'Asia/Calcutta')).toBeNull();
    expect(fieldClockName({ kind: 'series', tzid: null }, 'UTC')).toBeNull();
    expect(fieldClockName(NEW_YORK, BERLIN)).toBe('New York');
    expect(fieldClockName({ kind: 'series', tzid: null }, BERLIN)).toBe('UTC');
  });

  it('names the clock in the field labels', () => {
    expect(clockFieldLabel('startTime', NEW_YORK, BERLIN, de)).toBe('Startzeit (New York)');
    expect(clockFieldLabel('endDate', { kind: 'series', tzid: null }, BERLIN, de)).toBe('Enddatum (UTC)');
    expect(clockFieldLabel('startDate', { kind: 'series', tzid: BERLIN }, BERLIN, de)).toBe('Startdatum');
  });
});

describe('the hint after the end time', () => {
  it('says the times on this device where the clocks differ', () => {
    expect(deviceHint(nineInNewYork(), BERLIN, 'de', de)).toEqual({
      label: 'Auf diesem Gerät (Berlin)',
      value: 'Montag, 14. September, 15:00 bis 16:00',
    });
  });

  it("names the end's day where it is another", () => {
    const late = fieldsAt('2026-09-14T21:30:00.000Z', '2026-09-14T22:30:00.000Z', NEW_YORK);
    expect(deviceHint(late, BERLIN, 'de', de)?.value).toBe(
      'Montag, 14. September, 23:30 bis Dienstag, 15. September, 00:30',
    );
  });

  it('is not there where the clocks agree, or the fields name no instant', () => {
    expect(deviceHint(fieldsAt('2026-09-14T13:00:00.000Z', '2026-09-14T14:00:00.000Z', { kind: 'series', tzid: BERLIN }), BERLIN, 'de', de)).toBeNull();
    const inTheGap = changeClockField(
      fieldsAt('2027-03-14T13:00:00.000Z', '2027-03-14T14:00:00.000Z', NEW_YORK),
      'startTime',
      '02:30',
    );
    expect(deviceHint(inTheGap, BERLIN, 'de', de)).toBeNull();
  });
});

describe('the sentences about the zone', () => {
  it('each stand on their own, labelled with the choice', () => {
    expect(
      zoneSentences(
        view({ unusable: 'Europe/Kyiv', calendarRefuses: 'Asia/Kolkata', notStorable: 'America/New_York' }),
        de,
      ),
    ).toEqual([
      {
        label: 'Zeitzone der Serie',
        value: 'Dieses Gerät kann die Zeitzone Kyiv nicht verwenden; die Zeiten stehen in UTC.',
      },
      {
        label: 'Zeitzone der Serie',
        value: 'Dieser Kalender kann die Zeitzone Kolkata nicht speichern; die Serie steht auf UTC.',
      },
      {
        label: 'Zeitzone der Serie',
        value:
          'Dieser Kalender kann die Zeitzone New York nicht speichern. Wähle eine andere Zeitzone oder einen anderen Kalender.',
      },
    ]);
    expect(zoneSentences(view({}), de)).toEqual([]);
  });

  it('say the series\' zone in the choice\'s place for "this and all following" (21a)', () => {
    expect(followingHint(view({ choice: 'hint' }), de)).toEqual({
      label: 'Zeitzone der Serie',
      value: 'New York. Ändern lässt sie sich, wenn du die ganze Serie bearbeitest.',
    });
    expect(followingHint(view({}), de)).toBeNull();
  });
});

describe('a typed time the clock skips', () => {
  it('says which time, on which day, on which clock', () => {
    const fields = changeClockField(
      fieldsAt('2026-03-29T08:00:00.000Z', '2026-03-29T09:00:00.000Z', { kind: 'series', tzid: BERLIN }),
      'startTime',
      '02:30',
    );
    expect(clockFieldGaps(fields, 'de', de)).toEqual({
      start: '02:30 gibt es am 29. März 2026 in Berlin nicht: Die Uhren werden vorgestellt. Wähle eine andere Uhrzeit.',
      // The end moved with the start, by the hour the start read before the change.
      end: null,
    });
  });
});

describe('what the repeat field says once it switched the clock (134a)', () => {
  it('says the device clock and the times when repeat goes off', () => {
    const off = keepingInstants(nineInNewYork(), DEVICE_CLOCK);
    expect(repeatClockAnnouncement(NEW_YORK, off, BERLIN, 'de', de)).toBe(
      'Keine Wiederholung. Die Zeiten stehen jetzt auf der Uhr dieses Geräts: Montag, 14. September, 15:00 bis 16:00.',
    );
  });

  it('says the series clock as a choice does when repeat comes on', () => {
    const on = keepingInstants(keepingInstants(nineInNewYork(), DEVICE_CLOCK), NEW_YORK);
    expect(repeatClockAnnouncement(DEVICE_CLOCK, on, BERLIN, 'de', de)).toBe(
      'Zeitzone der Serie: New York. Die Zeiten stehen jetzt auf dieser Uhr: Montag, 14. September, 09:00 bis 10:00.',
    );
  });

  it('says nothing where the clock the labels name stayed', () => {
    const berlin = { kind: 'series', tzid: BERLIN } as const;
    const fields = fieldsAt('2026-09-14T13:00:00.000Z', '2026-09-14T14:00:00.000Z', berlin);
    expect(repeatClockAnnouncement(DEVICE_CLOCK, fields, BERLIN, 'de', de)).toBeNull();
    expect(repeatClockAnnouncement(NEW_YORK, nineInNewYork(), BERLIN, 'de', de)).toBeNull();
  });

  it('leaves out the times where the fields name none on the new clock', () => {
    // Carried as typed into the hour New York skips on 14 March 2027.
    const carried: ClockFields = {
      clock: NEW_YORK,
      start: { date: '2027-03-14', time: '02:30', at: null, carried: true },
      end: { date: '2027-03-14', time: '03:30', at: null, carried: true },
    };
    expect(repeatClockAnnouncement(DEVICE_CLOCK, carried, BERLIN, 'de', de)).toBe(
      'Zeitzone der Serie: New York. Die Zeiten stehen jetzt auf dieser Uhr.',
    );
    // A pair that names an instant on the device's clock says it there.
    const off = fieldsOnView(carried, view({ choice: 'none', clock: DEVICE_CLOCK, zone: null }), 'instants');
    expect(off.clock).toEqual(DEVICE_CLOCK);
    // Read on the runtime's clock, so the day depends on where the test runs.
    expect(repeatClockAnnouncement(NEW_YORK, off, BERLIN, 'de', de)).toMatch(
      /^Keine Wiederholung\. Die Zeiten stehen jetzt auf der Uhr dieses Geräts: .+ bis .+\.$/,
    );
  });
});

describe('why a save is refused', () => {
  const form = openZoneForm('series', 'America/New_York', false);
  const unchanged = { times: false, rule: false };

  it('names a typed time the clock skips first', () => {
    const fields = changeClockField(nineInNewYork(), 'startTime', '02:30');
    const inTheGap = changeClockField(fields, 'startDate', '2027-03-14');
    expect(zoneSaveRefusal({ fields: inTheGap, view: view({}), form, changed: unchanged, language: 'de', t: de })).toBe(
      '02:30 gibt es am 14. März 2027 in New York nicht: Die Uhren werden vorgestellt. Wähle eine andere Uhrzeit.',
    );
  });

  it('refuses changed times of a zone this device cannot use, and lets an untouched one through', () => {
    const unusable = view({ unusable: 'Europe/Kyiv', clock: { kind: 'series', tzid: null } });
    const fields = nineInNewYork();
    expect(
      zoneSaveRefusal({ fields, view: unusable, form, changed: { times: true, rule: false }, language: 'de', t: de }),
    ).toBe(
      'Dieses Gerät kann die Zeitzone Kyiv nicht verwenden, deshalb lassen sich geänderte Zeiten oder eine geänderte Wiederholung nicht speichern. Wähle eine andere Zeitzone.',
    );
    expect(zoneSaveRefusal({ fields, view: unusable, form, changed: unchanged, language: 'de', t: de })).toBeNull();
  });

  it('refuses a zone the target calendar cannot store (139a)', () => {
    const moved = zoneFormView(form, {
      repeats: true,
      allDay: false,
      calendarZones: ['Europe/Berlin'],
      deviceZone: BERLIN,
      calendarMoved: true,
    });
    expect(
      zoneSaveRefusal({ fields: nineInNewYork(), view: moved, form, changed: unchanged, language: 'de', t: de }),
    ).toBe('Dieser Kalender kann die Zeitzone New York nicht speichern. Wähle eine andere Zeitzone oder einen anderen Kalender.');
  });
});
