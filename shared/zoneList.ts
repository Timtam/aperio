// The world list a series' zone is chosen from — this surface's doors into
// `cal_core::zone_list`.
//
// The names, the search, and where a stored or device zone stands are rules in
// the core and answer synchronously on both surfaces. The offsets need tzdata's
// rules: the phone asks its native core, the desktop asks its host (its
// WebAssembly module leaves chrono-tz out), so that one question is a Promise
// on both surfaces. An answer names the instant it is for; `offsetsAreFor`
// lets a form drop one for a start it no longer has.
//
// The words — a region's name, the offset's form ("UTC−04:00"), "(auch Oslo)",
// the count line — are this surface's, from its translations.

import type {
  ListedZone,
  RegionName,
  ZoneChoiceAnswer,
  ZoneHit,
  ZoneOffset,
  ZoneOffsets,
  ZoneOffsetsQuestion,
  ZoneRegion,
  ZoneSearchAnswer,
} from './types';

export type {
  ListedZone,
  RegionName,
  ZoneChoiceAnswer,
  ZoneHit,
  ZoneOffset,
  ZoneOffsets,
  ZoneOffsetsQuestion,
  ZoneRegion,
  ZoneSearchAnswer,
};

/** This surface's doors into `cal_core::zone_list`. */
export interface ZoneListRules {
  zoneLabelsJson(): string;
  zoneSearchJson(inputJson: string): string;
  zoneChoiceJson(inputJson: string): string;
  zoneOffsets(question: ZoneOffsetsQuestion): Promise<ZoneOffsets>;
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
 * and offset words ("+2", "UTC−3:30", "5:30") match only then.
 */
export function searchZones(
  query: string,
  regionNames: RegionName[],
  offsets: ZoneOffsets | null,
): ZoneSearchAnswer {
  return JSON.parse(
    rules().zoneSearchJson(JSON.stringify({ query, region_names: regionNames, offsets })),
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
export function zoneOffsets(at: string, today: string): Promise<ZoneOffsets> {
  return rules().zoneOffsets({ at, today });
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

/** A hit's text: its entry, and the other name it was found by. */
export function zoneHitText(hit: ZoneHit, offsets: ZoneOffsets | null, t: Translate): string {
  const entry =
    hit.entry.kind === 'utc'
      ? zoneEntryText(null, null, t)
      : zoneEntryText(
          listedZoneLabels()[hit.entry.position],
          offsets === null ? null : offsets.zones[hit.entry.position].offset,
          t,
        );
  return hit.also === null
    ? entry
    : t('dialogs.seriesTimeZone.alsoKnownAs', { entry, alias: hit.also.label });
}

/** The count line under the search: "Eine Zeitzone", "12 Zeitzonen", or that
 *  nothing matches. */
export function zoneCountText(count: number, query: string, t: Translate): string {
  return count === 0
    ? t('dialogs.seriesTimeZone.noMatch', { query })
    : t('dialogs.seriesTimeZone.count', { count });
}
