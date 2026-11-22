// The world list a series' zone is chosen from — this surface's doors into
// `cal_core::zone_list`.
//
// The names, the search, where a stored or device zone stands, the offsets and
// the groups (decision 118a) are rules in the core and answer synchronously on
// both surfaces: the phone asks its native core, the desktop its WebAssembly
// module, which carries the zones since stage 8. An offsets answer names the
// instant it is for; `offsetsAreFor` lets a form drop one for a start it no
// longer has.
//
// The words — a region's name, the offset's form ("UTC−04:00"), a group's
// clock and cities, "(auch Oslo)", the count line — are this surface's, from
// its translations.

import type {
  GroupClock,
  GroupPick,
  HeadEntry,
  ListedZone,
  RegionName,
  ZoneChoiceAnswer,
  ZoneGroup,
  ZoneGroups,
  ZoneGroupsQuestion,
  ZoneHit,
  ZoneOffset,
  ZoneOffsets,
  ZoneOffsetsQuestion,
  ZoneRefusal,
  ZoneRegion,
  ZoneSearchAnswer,
} from './types';

export type {
  GroupClock,
  GroupPick,
  HeadEntry,
  ListedZone,
  RegionName,
  ZoneChoiceAnswer,
  ZoneGroup,
  ZoneGroups,
  ZoneGroupsQuestion,
  ZoneHit,
  ZoneOffset,
  ZoneOffsets,
  ZoneOffsetsQuestion,
  ZoneRefusal,
  ZoneRegion,
  ZoneSearchAnswer,
};

/** This surface's doors into `cal_core::zone_list`, all synchronous. */
export interface ZoneListRules {
  zoneLabelsJson(): string;
  zoneSearchJson(inputJson: string): string;
  zoneChoiceJson(inputJson: string): string;
  zoneOffsetsJson(inputJson: string): string;
  zoneGroupsJson(inputJson: string): string;
}

type Translate = (key: string, vars?: Record<string, unknown>) => string;

let installedRules: ZoneListRules | null = null;
let labels: readonly ListedZone[] | null = null;

/** Bind this surface's doors into the core. */
export function installZoneListRules(rules: ZoneListRules): void {
  installedRules = rules;
  labels = null;
}

function rules(): ZoneListRules {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the list here is the copy the core
    // exists to replace.
    throw new Error(
      'zone list rules used before installZoneListRules() — the surface must ' +
        'bind its doors into cal_core::zone_list at startup',
    );
  }
  return installedRules;
}

/** Every region, in the order the core names them. */
export const ZONE_REGIONS: readonly ZoneRegion[] = [
  'africa',
  'america',
  'antarctica',
  'asia',
  'atlantic',
  'australia',
  'europe',
  'indian',
  'pacific',
];

/** Every listed zone, named in parts, by position. Read once per install and
 *  frozen: every position the core hands out indexes this one array. */
export function listedZoneLabels(): readonly ListedZone[] {
  if (labels === null) {
    labels = Object.freeze(JSON.parse(rules().zoneLabelsJson()) as ListedZone[]);
  }
  return labels;
}

/**
 * A zone as a sentence names it: its city, with the area of a three-part id
 * ("Vancouver", "Indianapolis (Indiana)"), from the core's world list. A
 * zone the list does not hold, or a surface that has not bound the list (a
 * test), reads as its name, never as a guess.
 */
export function zoneCityText(zone: string): string {
  let listed: readonly ListedZone[];
  try {
    listed = listedZoneLabels();
  } catch {
    return zone;
  }
  const hit = listed.find((entry) => entry.zone === zone);
  if (!hit) return zone;
  return hit.area === null ? hit.city : `${hit.city} (${hit.area})`;
}

/** A region's name in this surface's language. */
export function zoneRegionName(region: ZoneRegion, t: Translate): string {
  return t(`dialogs.seriesTimeZone.region.${region}`);
}

/** Every region's name, for a search question. */
export function zoneRegionNames(t: Translate): RegionName[] {
  return ZONE_REGIONS.map((region) => ({ region, name: zoneRegionName(region, t) }));
}

/**
 * Search the list. Every word must match; a query of nothing but spaces
 * matches every entry. Hits come in the list's order when `offsets` are given,
 * and offset words ("+2", "UTC−3:30", "5:30") match only then. With `groups`
 * a query of offsets alone, or of nothing, finds the UTC entry and groups by
 * the offsets their entries say (121a; the answer's `kind` is `groups`); any
 * other word finds cities.
 */
export function searchZones(
  query: string,
  regionNames: RegionName[],
  offsets: ZoneOffsets | null,
  groups: ZoneGroups | null = null,
): ZoneSearchAnswer {
  return JSON.parse(
    rules().zoneSearchJson(
      JSON.stringify({ query, region_names: regionNames, offsets, groups }),
    ),
  ) as ZoneSearchAnswer;
}

/** Where a stored zone and the device's zone stand in the list. */
export function zoneChoice(
  stored: string | null | undefined,
  device: string | null | undefined,
): ZoneChoiceAnswer {
  return JSON.parse(
    rules().zoneChoiceJson(JSON.stringify({ stored: stored ?? null, device: device ?? null })),
  ) as ZoneChoiceAnswer;
}

/** Every listed zone's offset at `at` and standard offset as of `today`, and
 *  the list's order. Both instants are RFC 3339. */
export function zoneOffsets(at: string, today: string): ZoneOffsets {
  const question: ZoneOffsetsQuestion = { at, today };
  return JSON.parse(rules().zoneOffsetsJson(JSON.stringify(question))) as ZoneOffsets;
}

/**
 * The list as groups of zones with the same rules from `today` on (decision
 * 118a), the rows above it, and every listed zone's marks. `calendarZones` is
 * the calendar's manifest list (97a); `unusable` the listed names this
 * device's clock library refuses.
 */
export function zoneGroups(question: ZoneGroupsQuestion): ZoneGroups {
  return JSON.parse(rules().zoneGroupsJson(JSON.stringify(question))) as ZoneGroups;
}

/** Whether `offsets` were asked for `at`, compared as instants. */
export function offsetsAreFor(offsets: ZoneOffsets | null, at: string): boolean {
  return offsets !== null && Date.parse(offsets.at) === Date.parse(at);
}

/** An offset in this surface's form: "UTC+02:00", "UTC−04:00". */
export function zoneOffsetText(offset: ZoneOffset, t: Translate): string {
  return t('dialogs.seriesTimeZone.offset', {
    sign: t(`dialogs.seriesTimeZone.offsetSign.${offset.sign}`),
    hh: offset.hh,
    mm: offset.mm,
  });
}

/**
 * An entry's text: "Berlin, Europa, UTC+02:00", "Indianapolis (Indiana),
 * Amerika, UTC−04:00", or the UTC entry for `zone` null. Without an offset yet
 * the entry stands without one.
 */
export function zoneEntryText(
  zone: ListedZone | null,
  offset: ZoneOffset | null,
  t: Translate,
): string {
  if (zone === null) return t('dialogs.seriesTimeZone.utcEntry');
  const key =
    zone.area === null
      ? offset
        ? 'entry'
        : 'entryNoOffset'
      : offset
        ? 'entryWithArea'
        : 'entryWithAreaNoOffset';
  return t(`dialogs.seriesTimeZone.${key}`, {
    city: zone.city,
    area: zone.area ?? '',
    region: zoneRegionName(zone.region, t),
    offset: offset ? zoneOffsetText(offset, t) : '',
  });
}

/**
 * A group's clock as its entry says it (125a): "UTC+05:30, ohne Sommerzeit",
 * "UTC+01:00, Sommerzeit UTC+02:00", "UTC−04:00, ab 7. November 2027
 * UTC−05:00". `formatDay` writes a date (`YYYY-MM-DD`) in this surface's
 * language.
 */
export function groupClockText(
  clock: GroupClock,
  t: Translate,
  formatDay: (day: string) => string,
): string {
  switch (clock.kind) {
    case 'fixed':
      return t('dialogs.seriesTimeZone.clockFixed', { offset: zoneOffsetText(clock.offset, t) });
    case 'seasonal':
      return t('dialogs.seriesTimeZone.clockSeasonal', {
        standard: zoneOffsetText(clock.standard, t),
        summer: zoneOffsetText(clock.summer, t),
      });
    case 'changing':
      return t('dialogs.seriesTimeZone.clockChanging', {
        now: zoneOffsetText(clock.now, t),
        day: formatDay(clock.next_on),
        next: zoneOffsetText(clock.next, t),
      });
  }
}

/**
 * A group's entry, its clock first (119b): "UTC+01:00, Sommerzeit UTC+02:00:
 * Berlin, Paris, Rome und 13 weitere". The first city is the one choosing the
 * group stores. A group that names one city and no more reads as that city:
 * "UTC+05:45, ohne Sommerzeit: Kathmandu, Asien".
 */
export function zoneGroupText(
  group: ZoneGroup,
  t: Translate,
  formatDay: (day: string) => string,
): string {
  const labels = listedZoneLabels();
  const clock = groupClockText(group.clock, t, formatDay);
  const city = (position: number) => {
    const label = labels[position];
    return label.area === null ? label.city : `${label.city} (${label.area})`;
  };
  if (group.named.length === 1 && group.more === 0) {
    const only = labels[group.named[0]];
    return t('dialogs.seriesTimeZone.groupEntryOneCity', {
      clock,
      city: city(group.named[0]),
      region: zoneRegionName(only.region, t),
    });
  }
  const named = group.named.map(city).join(', ');
  const cities =
    group.more === 0
      ? named
      : t('dialogs.seriesTimeZone.groupCitiesMore', { cities: named, count: group.more });
  return t('dialogs.seriesTimeZone.groupEntry', { clock, cities });
}

/** A hit's text: its entry, and the other name it was found by. A group hit
 *  needs the groups the search was asked with. */
export function zoneHitText(
  hit: ZoneHit,
  offsets: ZoneOffsets | null,
  t: Translate,
  groups: ZoneGroups | null = null,
  formatDay: (day: string) => string = (day) => day,
): string {
  const found = hit.entry;
  let entry: string;
  switch (found.kind) {
    case 'utc':
      entry = zoneEntryText(null, null, t);
      break;
    case 'zone':
      entry = zoneEntryText(
        listedZoneLabels()[found.position],
        offsets === null ? null : offsets.zones[found.position].offset,
        t,
      );
      break;
    case 'group':
      if (groups === null) {
        throw new Error('a group hit is worded from the groups the search was asked with');
      }
      entry = zoneGroupText(groups.groups[found.group], t, formatDay);
      break;
  }
  return hit.also === null
    ? entry
    : t('dialogs.seriesTimeZone.alsoKnownAs', { entry, alias: hit.also.label });
}

/**
 * The count line under the search. For cities: "Eine Zeitzone", "12
 * Zeitzonen". For groups (122b, 126a): "62 Gruppen" for the whole list,
 * "8 Gruppen" or "UTC und 4 Gruppen" for an offset search. Nothing found:
 * that nothing matches.
 */
export function zoneCountText(
  count: number,
  query: string,
  t: Translate,
  answer: ZoneSearchAnswer | null = null,
): string {
  if (count === 0) return t('dialogs.seriesTimeZone.noMatch', { query });
  if (answer === null || answer.kind === 'zones') {
    return t('dialogs.seriesTimeZone.count', { count });
  }
  const groups = answer.hits.filter((hit) => hit.entry.kind === 'group').length;
  const withUtc = query.trim() !== '' && answer.hits.some((hit) => hit.entry.kind === 'utc');
  return withUtc
    ? t('dialogs.seriesTimeZone.groupCountWithUtc', { count: groups })
    : t('dialogs.seriesTimeZone.groupCount', { count: groups });
}
