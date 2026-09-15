import { describe, expect, it } from 'vitest';

import {
  listedZoneLabels,
  searchZones,
  zoneCountText,
  zoneEntryText,
  zoneHitText,
  zoneOffsetText,
  zoneRegionNames,
  type ZoneOffset,
  type ZoneOffsets,
} from '@aperio/shared';
import i18n from '../i18n';

const de = i18n.getFixedT('de');
const en = i18n.getFixedT('en');

const offset = (seconds: number, sign: 'plus' | 'minus', hh: string, mm: string): ZoneOffset => ({
  seconds,
  sign,
  hh,
  mm,
});

const zone = (id: string) => {
  const found = listedZoneLabels().find((label) => label.zone === id);
  if (!found) throw new Error(`${id} is not listed`);
  return found;
};

/** Offsets for every listed zone, with `seconds` for the one asked about. */
const offsetsWith = (id: string, value: ZoneOffset): ZoneOffsets => {
  const labels = listedZoneLabels();
  return {
    at: '2026-09-14T12:00:00Z',
    today: '2026-09-14T12:00:00Z',
    zones: labels.map((label) =>
      label.zone === id
        ? { offset: value, standard: value }
        : { offset: offset(0, 'plus', '00', '00'), standard: offset(0, 'plus', '00', '00') },
    ),
    order: [{ kind: 'utc' }, ...labels.map((_, position) => ({ kind: 'zone' as const, position }))],
  };
};

describe('the zone list in words', () => {
  it('writes an offset with the real minus sign, as Toni chose (form F)', () => {
    expect(zoneOffsetText(offset(-14400, 'minus', '04', '00'), de)).toBe('UTC−04:00');
    expect(zoneOffsetText(offset(19800, 'plus', '05', '30'), en)).toBe('UTC+05:30');
  });

  it('writes an entry by its parts, with the area of a three-part id', () => {
    expect(zoneEntryText(zone('Europe/Berlin'), offset(7200, 'plus', '02', '00'), de)).toBe(
      'Berlin, Europa, UTC+02:00',
    );
    expect(
      zoneEntryText(zone('America/Indiana/Indianapolis'), offset(-14400, 'minus', '04', '00'), de),
    ).toBe('Indianapolis (Indiana), Amerika, UTC−04:00');
    expect(zoneEntryText(zone('Indian/Maldives'), null, en)).toBe('Maldives, Indian Ocean');
    expect(zoneEntryText(null, null, de)).toBe('UTC (ohne Sommerzeit)');
  });

  it('names the other name a search found an entry by', () => {
    const { hits } = searchZones('Oslo', zoneRegionNames(de), null);
    expect(hits).toHaveLength(1);
    const offsets = offsetsWith('Europe/Berlin', offset(7200, 'plus', '02', '00'));
    expect(zoneHitText(hits[0], offsets, de)).toBe('Berlin, Europa, UTC+02:00 (auch Oslo)');
    expect(zoneHitText(hits[0], null, en)).toBe('Berlin, Europe (also Oslo)');
  });

  it('finds a region by its name in the surface language', () => {
    const berlin = listedZoneLabels().findIndex((label) => label.zone === 'Europe/Berlin');
    const hit = (query: string, t: typeof de) =>
      searchZones(query, zoneRegionNames(t), null).hits.some(
        (h) => h.entry.kind === 'zone' && h.entry.position === berlin && h.via[0] === 'region',
      );
    expect(hit('Europa', de)).toBe(true);
    expect(hit('Europa', en)).toBe(false);
  });

  it('counts, and says when nothing matches', () => {
    expect(zoneCountText(1, 'berlin', de)).toBe('Eine Zeitzone');
    expect(zoneCountText(12, 'b', de)).toBe('12 Zeitzonen');
    expect(zoneCountText(0, 'xyz', de)).toBe('Keine Zeitzone passt zu „xyz“.');
    expect(zoneCountText(12, 'b', en)).toBe('12 time zones');
  });

  // The desktop's offsets come from its host, which the tests do not run; these
  // hand-made ones stand in, so the WebAssembly door is asked with the offsets
  // a search carries.
  it('matches an offset word against the offsets the search is handed', () => {
    const offsets = offsetsWith('Europe/Berlin', offset(7200, 'plus', '02', '00'));
    const { hits } = searchZones('berlin +2', zoneRegionNames(de), offsets);
    expect(hits.map((hit) => [zoneHitText(hit, offsets, de), hit.via])).toEqual([
      ['Berlin, Europa, UTC+02:00', ['city', 'offset']],
    ]);
    expect(searchZones('berlin +1', zoneRegionNames(de), offsets).hits).toEqual([]);
  });

  it('refuses, through the door, a search without every region name', () => {
    let refusal: unknown = null;
    try {
      searchZones('berlin', zoneRegionNames(de).slice(1), null);
    } catch (error) {
      refusal = error;
    }
    expect(String(refusal)).toMatch(/no name for the region/);
  });
});
