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
const LISTED = listedZoneLabels().map((label) => label.zone);

/** The rows as a surface lays them out. Without `unusable`, the names this
 *  runtime refuses, read as the surfaces read them (`unusableZones`); with it,
 *  another device's, such as old Android's. */
function rows(
  query: string,
  {
    stored = null,
    device = 'Europe/Berlin',
    calendar = 'all',
    unusable = unusableZones(LISTED, stored),
  }: {
    stored?: string | null;
    device?: string | null;
    calendar?: 'all' | 'none' | string[];
    unusable?: string[];
  } = {},
) {
  const groups = zoneGroups({ today: TODAY, stored, device, calendar_zones: calendar, unusable });
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
    // The surfaces count this spelling as one the device refuses; the row
    // still gives its own reason alone, and keeping it stays a choice.
    expect(unusableZones(LISTED, 'Pacific Standard Time')).toContain('Pacific Standard Time');
    const unreadable = rows('', { stored: 'Pacific Standard Time' });
    expect(row(unreadable, 'head-current')).toMatchObject({
      text: 'Aktuell: Pacific Standard Time (Aperio kann diese Zeitzone nicht lesen; die Serie wiederholt sich nach UTC)',
      selectable: true,
      pick: { kind: 'unchanged' },
    });
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

  it('says the name the UTC entry was found by', () => {
    expect(rows('gmt').rows.map((r) => r.text)).toEqual(['UTC (ohne Sommerzeit) (auch GMT)']);
  });

  it('changes a stored spelling this device refuses, from every row naming it', () => {
    // Old Android in Kyiv: it reports Europe/Kiev and refuses Europe/Kyiv.
    const options = { stored: 'Europe/Kyiv', device: 'Europe/Kiev', unusable: ['Europe/Kyiv'] };
    const empty = rows('', options);
    expect(empty.rows.some((r) => r.key === 'head-current')).toBe(false);
    // The device's row stands in for "Aktuell": it says the series' zone,
    // and choosing it writes the spelling this device can use.
    expect(row(empty, 'head-device')).toMatchObject({
      pick: { kind: 'zone', zone: 'Europe/Kiev' },
      current: true,
      selectable: true,
    });
    expect(groupWith(empty, 'Kyiv').pick).toEqual({ kind: 'zone', zone: 'Europe/Kiev' });
    const hit = rows('kyiv', options).rows[0];
    expect(hit).toMatchObject({ pick: { kind: 'zone', zone: 'Europe/Kiev' }, current: false, selectable: true });
  });

  it('keeps a stored spelling this device can use, though it refuses the listed name', () => {
    // Old Android in Berlin: Europe/Kiev works, Europe/Kyiv does not.
    const options = { stored: 'Europe/Kiev', unusable: ['Europe/Kyiv'] };
    const hit = rows('kyiv', options).rows[0];
    expect(hit).toMatchObject({ pick: { kind: 'unchanged' }, current: true, selectable: true });
    expect(hit.text).toBe('Kyiv, Europa, UTC+02:00');
    expect(row(rows('', options), 'head-current').text).toBe('Aktuell: Europe/Kiev (nicht in Aperios Liste)');
  });

  it('marks "Aktuell" where this device refuses the stored spelling', () => {
    const current = row(rows('', { stored: 'Europe/Kyiv', unusable: ['Europe/Kyiv'] }), 'head-current');
    expect(current).toMatchObject({
      text: 'Aktuell: Kyiv, Europa, UTC+02:00 (dieses Gerät kann die Zeitzone nicht verwenden)',
      selectable: false,
    });
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
    const calendar = LISTED.filter((zone) => zone !== 'Europe/Vienna');
    const list = rows('vienna', { calendar });
    expect(list.rows[0]).toMatchObject({
      text: 'Vienna, Europa, UTC+01:00 (dieser Kalender kann diese Zeitzone nicht speichern)',
      selectable: false,
    });
  });

  it("keep the calendar's refusal on the zone in effect, as its group does", () => {
    // The series stores Vienna; the calendar it moved to cannot.
    const options = { stored: 'Europe/Vienna', calendar: LISTED.filter((zone) => zone !== 'Europe/Vienna') };
    expect(rows('vienna', options).rows[0]).toMatchObject({
      text: 'Vienna, Europa, UTC+01:00 (dieser Kalender kann diese Zeitzone nicht speichern)',
      selectable: false,
      current: false,
    });
    const empty = rows('', options);
    expect(row(empty, 'head-current')).toMatchObject({
      text: 'Aktuell: Vienna, Europa, UTC+01:00 (dieser Kalender kann diese Zeitzone nicht speichern)',
      selectable: false,
    });
    // The group names only cities it can store (127a), and moves to the device's.
    const group = groupWith(empty, 'Berlin');
    expect(group.text).not.toContain('Vienna');
    expect(group).toMatchObject({ selectable: true, current: false });
    expect(group.pick).toEqual({ kind: 'zone', zone: 'Europe/Berlin' });
  });

  it("still says the zone in effect on the device's row where the calendar refuses it", () => {
    // "Aktuell" is left out for the device's row, which says the series' zone
    // even marked off, so the list still tells which zone the series is on.
    const options = { stored: 'Europe/Berlin', calendar: LISTED.filter((zone) => zone !== 'Europe/Berlin') };
    const empty = rows('', options);
    expect(empty.rows.some((r) => r.key === 'head-current')).toBe(false);
    expect(row(empty, 'head-device')).toMatchObject({
      text: 'Dieses Gerät: Berlin, Europa, UTC+01:00 (dieser Kalender kann diese Zeitzone nicht speichern)',
      selectable: false,
      current: true,
    });
  });

  it('mark "Aktuell" for a stored zone outside the list the calendar cannot store', () => {
    // Exchange's list has no EST5EDT; a calendar of the listed zones alone
    // stores no zone outside the list.
    const current = row(rows('', { stored: 'EST5EDT', calendar: LISTED }), 'head-current');
    expect(current).toMatchObject({
      text: 'Aktuell: EST5EDT (nicht in Aperios Liste) (dieser Kalender kann diese Zeitzone nicht speichern)',
      selectable: false,
      current: true,
    });
    expect(row(rows('', { stored: 'EST5EDT', device: 'EST5EDT' }), 'head-device')).toMatchObject({
      pick: { kind: 'unchanged' },
      current: true,
      selectable: true,
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
