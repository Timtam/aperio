import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/timeZoneFilter.json';
import {
  listedZoneLabels,
  searchZones,
  zoneChoice,
  zoneCountText,
  zoneGroups,
  zoneGroupText,
  zoneOffsets,
  type RegionName,
  type ZoneGroups,
  type ZoneGroupsQuestion,
  type ZoneHit,
  type ZoneRegion,
} from '@aperio/shared';
import i18n from '../i18n';

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

interface GroupRow {
  of: string;
  today?: string;
  stored?: string;
  device?: string;
  calendar?: 'all' | 'none' | { except: string[] };
  unusable?: string[];
  main?: string;
  named?: string[];
  more?: number;
  standard?: number;
  pick?: { zone: string; source: string } | null;
  refusal?: string | null;
  note: string;
}

interface GroupSearchRow {
  today: string;
  query: string;
  offsets?: { at: string; today: string };
  kind: 'groups' | 'zones';
  hits?: (string | HitRow)[];
  count?: number;
  note: string;
}

interface HeadRow {
  stored?: string;
  device?: string;
  calendar?: 'all' | 'none' | { except: string[] };
  expect: { kind: string }[];
  note: string;
}

interface OffsetRow {
  zone: string;
  at: string;
  today: string;
  seconds: number;
  standard?: number;
}

const regions = contract.regions as unknown as Record<'de' | 'en', Record<ZoneRegion, string>>;
const searchRows = contract.search as unknown as SearchRow[];
const choiceRows = contract.choice as unknown as ChoiceRow[];
const offsetRows = contract.offsets as unknown as OffsetRow[];
const groupRows = contract.groups as unknown as GroupRow[];
const groupSearchRows = contract.groupSearch as unknown as GroupSearchRow[];
const headRows = contract.head as unknown as HeadRow[];
const mainRows = contract.groupMains as unknown as { today: string; mains: string[] }[];

const TODAY = '2026-11-22T12:00:00Z';

const groupsFor = (row: {
  today?: string;
  stored?: string;
  device?: string;
  calendar?: 'all' | 'none' | { except: string[] };
  unusable?: string[];
}): ZoneGroups => {
  const calendar = row.calendar ?? 'all';
  const question: ZoneGroupsQuestion = {
    today: row.today ?? TODAY,
    stored: row.stored ?? null,
    device: row.device ?? null,
    calendar_zones:
      typeof calendar === 'string'
        ? calendar
        : listedZoneLabels()
            .map((label) => label.zone)
            .filter((zone) => !calendar.except.includes(zone)),
    unusable: row.unusable ?? [],
  };
  return zoneGroups(question);
};

const zoneName = (position: number): string => listedZoneLabels()[position].zone;

const regionNames = (set: 'de' | 'en'): RegionName[] =>
  (Object.entries(regions[set]) as [ZoneRegion, string][]).map(([region, name]) => ({
    region,
    name,
  }));

const zoneOf = (entry: ZoneHit['entry'], groups: ZoneGroups | null = null): string => {
  switch (entry.kind) {
    case 'utc':
      return 'utc';
    case 'zone':
      return zoneName(entry.position);
    case 'group':
      return groups === null
        ? `group ${entry.group}`
        : `group:${zoneName(groups.groups[entry.group].members[0])}`;
  }
};

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
 * the path the desktop takes, offsets and groups included since they come
 * from the module. The core reads every row in
 * `crates/cal-core/src/zone_list.rs`, and the phone's doors in
 * `crates/cal-ffi/src/lib.rs`.
 */
describe('zoneList contract (WebAssembly door and shared shell)', () => {
  // Anti-silence: named rows, not a count.
  it('still carries the rows the rules turn on', () => {
    const queries = new Set(searchRows.map((row) => row.query));
    // The rows without offsets the core and the phone's door name too.
    for (const query of [
      'kiev',
      'Oslo',
      'Montreal',
      'Zuerich',
      'Wien',
      'Europa',
      'GMT',
      '   ',
      'berlin europa',
      'enix',
      'Europe/Kiev',
    ]) {
      expect(queries, `table lost ${JSON.stringify(query)}`).toContain(query);
    }
    expect(choiceRows.some((row) => row.stored === 'Europe/Kiev')).toBe(true);
    expect(choiceRows.some((row) => row.device === 'Asia/Calcutta')).toBe(true);
    const labelled = contract.labels.map((row) => row.zone);
    for (const zone of ['Europe/Berlin', 'America/Indiana/Indianapolis']) {
      expect(labelled, `table lost the ${zone} label`).toContain(zone);
    }
    for (const name of ['EST5EDT', 'Europe/Oslo']) {
      expect(contract.notListed, `table lost ${name} from notListed`).toContain(name);
    }
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

  for (const row of searchRows) {
    it(`searches ${JSON.stringify(row.query)} (${row.regions}${row.offsets ? ', with offsets' : ''})`, () => {
      const offsets = row.offsets ? zoneOffsets(row.offsets.at, row.offsets.today) : null;
      const { hits } = searchZones(row.query, regionNames(row.regions), offsets);
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
  for (const row of offsetRows) {
    it(`gives ${row.zone} its offset at ${row.at}`, () => {
      const offsets = zoneOffsets(row.at, row.today);
      const position = listedZoneLabels().findIndex((label) => label.zone === row.zone);
      expect(offsets.zones[position].offset.seconds).toBe(row.seconds);
      if (row.standard !== undefined) {
        expect(offsets.zones[position].standard.seconds).toBe(row.standard);
      }
    });
  }

  for (const row of groupRows) {
    it(`groups ${row.of}: ${row.note}`, () => {
      const answer = groupsFor(row);
      const group = answer.groups.find((candidate) =>
        candidate.members.some((member) => zoneName(member) === row.of),
      );
      expect(group, `${row.of} is in no group`).toBeDefined();
      if (group === undefined) return;
      if (row.main !== undefined) expect(zoneName(group.members[0])).toBe(row.main);
      if (row.named !== undefined) expect(group.named.map(zoneName)).toEqual(row.named);
      if (row.more !== undefined) expect(group.more).toBe(row.more);
      if (row.standard !== undefined) expect(group.standard.seconds).toBe(row.standard);
      if (row.refusal !== undefined) expect(group.refusal).toBe(row.refusal);
      if (row.pick !== undefined) {
        expect(
          group.pick === null ? null : { zone: zoneName(group.pick.position), source: group.pick.source },
        ).toEqual(row.pick);
      }
    });
  }

  for (const row of mainRows) {
    it(`names the main city of every group on ${row.today}`, () => {
      const mains = groupsFor({ today: row.today })
        .groups.filter((group) => group.members.length > 1)
        .map((group) => zoneName(group.members[0]));
      expect(mains).toEqual(row.mains);
    });
  }

  for (const row of groupSearchRows) {
    it(`searches ${JSON.stringify(row.query)} with groups on ${row.today}`, () => {
      const groups = groupsFor(row);
      const offsets = row.offsets ? zoneOffsets(row.offsets.at, row.offsets.today) : null;
      const answer = searchZones(row.query, regionNames('en'), offsets, groups);
      expect(answer.kind, row.note).toBe(row.kind);
      if (row.hits !== undefined) {
        expect(answer.hits.length, row.note).toBe(row.hits.length);
        row.hits.forEach((want, i) => {
          if (typeof want === 'string') {
            expect(zoneOf(answer.hits[i].entry, groups), row.note).toBe(want);
          } else {
            expect(hitIs(answer.hits[i], want), row.note).toBe(true);
          }
        });
      }
      if (row.count !== undefined) expect(answer.hits.length, row.note).toBe(row.count);
    });
  }

  for (const row of headRows) {
    it(`puts the rows above the list: ${row.note}`, () => {
      expect(groupsFor(row).head.map((entry) => entry.kind)).toEqual(
        row.expect.map((entry) => entry.kind),
      );
    });
  }
});

/**
 * How a group is heard (119b, 122b, 125a and 126a): its clock
 * first, then up to three cities and how many more; a group of one city reads
 * as that city; the count line counts groups.
 */
describe('zone group wording', () => {
  const de = i18n.getFixedT('de');
  const en = i18n.getFixedT('en');
  const day = (iso: string) => iso;
  const groupOf = (answer: ZoneGroups, zone: string) => {
    const group = answer.groups.find((candidate) =>
      candidate.members.some((member) => zoneName(member) === zone),
    );
    if (group === undefined) throw new Error(`${zone} is in no group`);
    return group;
  };

  it('says the clock first, then the cities', () => {
    const answer = groupsFor({});
    expect(zoneGroupText(groupOf(answer, 'Europe/Berlin'), de, day)).toBe(
      'UTC+01:00, Sommerzeit UTC+02:00: Berlin, Paris, Rome und 13 weitere',
    );
    expect(zoneGroupText(groupOf(answer, 'Africa/Johannesburg'), de, day)).toBe(
      'UTC+02:00, ohne Sommerzeit: Johannesburg, Khartoum, Maputo und 4 weitere',
    );
    expect(zoneGroupText(groupOf(answer, 'Asia/Kolkata'), de, day)).toBe(
      'UTC+05:30, ohne Sommerzeit: Kolkata, Colombo',
    );
    expect(zoneGroupText(groupOf(answer, 'Europe/Berlin'), en, day)).toBe(
      'UTC+01:00, daylight saving time UTC+02:00: Berlin, Paris, Rome and 13 more',
    );
  });

  it('reads a group of one city as that city, with its region', () => {
    expect(zoneGroupText(groupOf(groupsFor({}), 'Asia/Kathmandu'), de, day)).toBe(
      'UTC+05:45, ohne Sommerzeit: Kathmandu, Asien',
    );
  });

  it('says a coming change with its day', () => {
    const answer = groupsFor({ today: '2026-10-26T12:00:00Z' });
    expect(zoneGroupText(groupOf(answer, 'America/Havana'), de, () => '7. November 2027')).toBe(
      'UTC−04:00, ab 7. November 2027 UTC−05:00: Havana, Amerika',
    );
  });

  it('says "eine weitere" for one more city', () => {
    const denver = groupOf(groupsFor({}), 'America/Denver');
    expect(denver.more).toBe(1);
    expect(zoneGroupText(denver, de, day)).toBe(
      'UTC−07:00, Sommerzeit UTC−06:00: Denver, Boise, Ciudad Juarez und eine weitere',
    );
  });

  it('counts groups', () => {
    const groups = groupsFor({});
    const all = searchZones('   ', regionNames('de'), null, groups);
    expect(zoneCountText(all.hits.length, '   ', de, all)).toBe('62 Gruppen');
    const plusTwo = searchZones('+2', regionNames('de'), null, groups);
    expect(zoneCountText(plusTwo.hits.length, '+2', de, plusTwo)).toBe('8 Gruppen');
    const zero = searchZones('+0', regionNames('de'), null, groups);
    expect(zoneCountText(zero.hits.length, '+0', de, zero)).toBe('UTC und 4 Gruppen');
    const city = searchZones('Vienna', regionNames('de'), null, groups);
    expect(zoneCountText(city.hits.length, 'Vienna', de, city)).toBe('Eine Zeitzone');
  });
});
