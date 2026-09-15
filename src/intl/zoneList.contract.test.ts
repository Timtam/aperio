import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/timeZoneFilter.json';
import {
  listedZoneLabels,
  searchZones,
  zoneChoice,
  type RegionName,
  type ZoneHit,
  type ZoneRegion,
} from '@aperio/shared';

interface HitRow {
  zone: string;
  via: string[];
  also?: { label: string; name?: string; kind?: string };
}

interface SearchRow {
  query: string;
  regions: 'de' | 'en';
  offsets?: { at: string; today: string };
  hits?: HitRow[];
  count?: number;
  includes?: HitRow[];
  excludes?: string[];
  note: string;
}

interface ChoiceRow {
  stored: string | null;
  device?: string;
  expect: { stored: Record<string, string>; device?: Record<string, string> };
}

const regions = contract.regions as unknown as Record<'de' | 'en', Record<ZoneRegion, string>>;
const searchRows = contract.search as unknown as SearchRow[];
const choiceRows = contract.choice as unknown as ChoiceRow[];

const regionNames = (set: 'de' | 'en'): RegionName[] =>
  (Object.entries(regions[set]) as [ZoneRegion, string][]).map(([region, name]) => ({
    region,
    name,
  }));

const zoneOf = (entry: ZoneHit['entry']): string =>
  entry.kind === 'utc' ? 'utc' : listedZoneLabels()[entry.position].zone;

const hitIs = (hit: ZoneHit, want: HitRow): boolean =>
  zoneOf(hit.entry) === want.zone &&
  JSON.stringify(hit.via) === JSON.stringify(want.via) &&
  (want.also === undefined
    ? hit.also === null
    : hit.also !== null &&
      hit.also.label === want.also.label &&
      (want.also.name === undefined || hit.also.name === want.also.name) &&
      (want.also.kind === undefined || hit.also.kind === want.also.kind));

/**
 * The world zone list through the real WebAssembly door and the shared shell —
 * the path the desktop takes. Rows that need offsets are not run here: the
 * offsets need chrono-tz, which the module leaves out. The core reads every row
 * in `crates/cal-core/src/zone_list.rs`, and the phone's doors in
 * `crates/cal-ffi/src/lib.rs`.
 */
describe('zoneList contract (WebAssembly door and shared shell)', () => {
  // Anti-silence: named rows, not a count.
  it('still carries the rows the rules turn on', () => {
    const queries = new Set(searchRows.map((row) => row.query));
    for (const query of ['kiev', 'Oslo', 'Montreal', 'Zuerich', 'Wien', 'Europa', 'GMT', '   ']) {
      expect(queries, `table lost ${JSON.stringify(query)}`).toContain(query);
    }
    expect(choiceRows.some((row) => row.stored === 'Europe/Kiev')).toBe(true);
  });

  for (const row of contract.labels) {
    it(`names ${row.zone}`, () => {
      expect(listedZoneLabels().find((label) => label.zone === row.zone)).toEqual(row);
    });
  }

  it('lists none of the names that are not entries', () => {
    const listed = new Set(listedZoneLabels().map((label) => label.zone));
    for (const name of contract.notListed) {
      expect(listed.has(name), name).toBe(false);
    }
  });

  for (const row of searchRows.filter((row) => row.offsets === undefined)) {
    it(`searches ${JSON.stringify(row.query)} (${row.regions})`, () => {
      const { hits } = searchZones(row.query, regionNames(row.regions), null);
      if (row.hits !== undefined) {
        expect(hits.length, row.note).toBe(row.hits.length);
        row.hits.forEach((want, i) => expect(hitIs(hits[i], want), row.note).toBe(true));
      }
      if (row.count !== undefined) expect(hits.length, row.note).toBe(row.count);
      for (const want of row.includes ?? []) {
        expect(
          hits.some((hit) => hitIs(hit, want)),
          `${row.note} — missing ${JSON.stringify(want)}`,
        ).toBe(true);
      }
      for (const zone of row.excludes ?? []) {
        expect(hits.some((hit) => zoneOf(hit.entry) === zone), `${row.note} — ${zone}`).toBe(false);
      }
    });
  }

  for (const row of choiceRows) {
    it(`places stored ${JSON.stringify(row.stored)} and device ${JSON.stringify(row.device)}`, () => {
      const answer = zoneChoice(row.stored, row.device);
      const plain = (choice: Record<string, unknown>) => {
        const { position, ...rest } = choice as { position?: number };
        return position === undefined
          ? rest
          : { ...rest, zone: listedZoneLabels()[position].zone };
      };
      expect(plain(answer.stored as Record<string, unknown>)).toEqual(row.expect.stored);
      if (row.expect.device === undefined) {
        expect(answer.device).toBeNull();
      } else {
        expect(plain(answer.device as Record<string, unknown>)).toEqual(row.expect.device);
      }
    });
  }
});
