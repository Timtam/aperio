import { describe, expect, it } from 'vitest';

import {
  listedZoneLabels,
  unusableZones,
  zoneGroups,
  zoneOffsets,
  zonePickerList,
  zoneRegionNames,
  type ZonePickerRow,
} from '@aperio/shared';
import i18n from '../i18n';

/**
 * The series zone picker's rows (stage 11), shared by the desktop dialog and
 * the phone's: what each row says and what choosing it does. Every rule behind
 * them is the core's (zoneGroups, zoneSearch); these pin how the rows lay the
 * answers out, in German.
 */

const de = i18n.getFixedT('de');
const TODAY = '2026-11-22T12:00:00Z';
const START = '2026-11-22T23:30:00Z';
const day = (iso: string) => iso;

function rows(
  query: string,
  {
    stored = null,
    device = 'Europe/Berlin',
    calendar = 'all',
  }: { stored?: string | null; device?: string | null; calendar?: 'all' | 'none' | string[] } = {},
) {
  const groups = zoneGroups({ today: TODAY, stored, device, calendar_zones: calendar, unusable: [] });
  return zonePickerList({
    query,
    groups,
    offsets: zoneOffsets(START, TODAY),
    regionNames: zoneRegionNames(de),
    stored,
    device,
    t: de,
    formatDay: day,
  });
}

const row = (list: { rows: ZonePickerRow[] }, key: string) => {
  const found = list.rows.find((candidate) => candidate.key === key);
  if (!found) throw new Error(`no row ${key}`);
  return found;
};
const groupWith = (list: { rows: ZonePickerRow[] }, city: string) => {
  const found = list.rows.find((candidate) => candidate.key.startsWith('group-') && candidate.text.includes(city));
  if (!found) throw new Error(`no group naming ${city}`);
  return found;
};

describe('the rows above the list', () => {
  it('say the zone in effect, the device and UTC while the search is empty', () => {
    const list = rows('', { stored: 'Europe/Vienna' });
    expect(list.rows.slice(0, 3).map((r) => r.key)).toEqual(['head-current', 'head-device', 'head-utc']);
    expect(row(list, 'head-current')).toMatchObject({
      text: 'Aktuell: Vienna, Europa, UTC+01:00',
      pick: { kind: 'unchanged' },
      current: true,
    });
    expect(row(list, 'head-device')).toMatchObject({
      text: 'Dieses Gerät: Berlin, Europa, UTC+01:00',
      pick: { kind: 'zone', zone: 'Europe/Berlin' },
      current: false,
    });
    expect(list.count).toBe('62 Gruppen');
  });

  it('leave out "Aktuell" on UTC, where choosing UTC changes nothing', () => {
    const list = rows('');
    expect(list.rows.some((r) => r.key === 'head-current')).toBe(false);
    expect(row(list, 'head-utc')).toMatchObject({ pick: { kind: 'unchanged' }, current: true });
    expect(row(list, 'utc')).toMatchObject({ pick: { kind: 'unchanged' }, current: true });
  });

  it('are gone once something is typed', () => {
    expect(rows('+2').rows.some((r) => r.key.startsWith('head-'))).toBe(false);
  });

  it('say a zone outside the list and one Aperio cannot read', () => {
    const outside = rows('', { device: 'EST5EDT' });
    expect(row(outside, 'head-device')).toMatchObject({
      text: 'Dieses Gerät: EST5EDT (nicht in Aperios Liste)',
      pick: { kind: 'zone', zone: 'EST5EDT' },
    });
    const unreadable = rows('', { stored: 'Pacific Standard Time' });
    expect(row(unreadable, 'head-current').text).toBe(
      'Aktuell: Pacific Standard Time (Aperio kann diese Zeitzone nicht lesen; die Serie wiederholt sich nach UTC)',
    );
  });
});

describe('what choosing a row does', () => {
  it('keeps the series zone when its group is chosen (120a)', () => {
    const berlin = groupWith(rows('', { stored: 'Europe/Vienna' }), 'Vienna');
    expect(berlin).toMatchObject({ pick: { kind: 'unchanged' }, current: true });
    expect(berlin.text.startsWith('UTC+01:00, Sommerzeit UTC+02:00: Vienna, Berlin, Paris')).toBe(true);
  });

  it("gives the device's reported name for its own zone, so 117a can spell it", () => {
    const kolkata = groupWith(rows('', { device: 'Asia/Calcutta' }), 'Kolkata');
    expect(kolkata.pick).toEqual({ kind: 'zone', zone: 'Asia/Calcutta' });
    expect(row(rows('', { device: 'Asia/Calcutta' }), 'head-device').pick).toEqual({
      kind: 'zone',
      zone: 'Asia/Calcutta',
    });
  });

  it('gives the main city of a group outside the series and the device', () => {
    expect(groupWith(rows(''), 'London').pick).toEqual({ kind: 'zone', zone: 'Europe/London' });
  });

  it('never respells the zone in effect: a stored Europe/Kiev found as Kyiv is no change', () => {
    const list = rows('kyiv', { stored: 'Europe/Kiev' });
    expect(list.rows).toHaveLength(1);
    expect(list.rows[0]).toMatchObject({ pick: { kind: 'unchanged' }, current: true });
  });

  it('stores the city a merged place points to', () => {
    const list = rows('oslo');
    expect(list.rows[0].text).toBe('Berlin, Europa, UTC+01:00 (auch Oslo)');
    expect(list.rows[0].pick).toEqual({ kind: 'zone', zone: 'Europe/Berlin' });
  });
});

describe('marked rows', () => {
  it('say why a group cannot be chosen', () => {
    const london = groupWith(rows('', { calendar: 'none' }), 'London');
    expect(london.selectable).toBe(false);
    expect(london.text.endsWith('(dieser Kalender kann keine dieser Zeitzonen speichern)')).toBe(true);
    expect(row(rows('', { calendar: 'none' }), 'head-utc').selectable).toBe(true);
  });

  it('say why a city cannot be chosen', () => {
    const calendar = listedZoneLabels()
      .map((label) => label.zone)
      .filter((zone) => zone !== 'Europe/Vienna');
    const list = rows('vienna', { calendar });
    expect(list.rows[0]).toMatchObject({
      text: 'Vienna, Europa, UTC+01:00 (dieser Kalender kann diese Zeitzone nicht speichern)',
      selectable: false,
    });
  });
});

describe('unusableZones', () => {
  it('adds the stored spelling when the device refuses it', () => {
    const listed = listedZoneLabels().map((label) => label.zone);
    expect(unusableZones(listed, 'Pacific Standard Time')).toContain('Pacific Standard Time');
    expect(unusableZones(listed, 'Europe/Berlin')).not.toContain('Europe/Berlin');
    expect(unusableZones(listed, null)).toEqual(unusableZones(listed, 'Europe/Berlin'));
  });
});
