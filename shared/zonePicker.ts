// The rows of the series zone picker (DESIGN-series-time-zone.md, stage 11)
// and what choosing each one does, the same on both surfaces.
//
// Every rule is the core's: the groups and their picks (118a-129a), the
// search (121a), where the stored and the device's zone stand. This file only
// lays the answers out as rows and words them through the surface's
// translations, so the desktop dialog and the phone's read one list.

import type { HeadEntry, RegionName, ZoneGroups, ZoneOffsets, ZoneRefusal } from './types';
import {
  listedZoneLabels,
  searchZones,
  zoneChoice,
  zoneCountText,
  zoneEntryText,
  zoneGroupText,
  zoneHitText,
} from './zoneList';

type Translate = (key: string, vars?: Record<string, unknown>) => string;

/**
 * What choosing a row does: set the series' zone (`null` for UTC), or nothing,
 * because the row is the zone in effect. Choosing the zone in effect again
 * rewrites nothing, not even its spelling (120a): a stored `Europe/Kiev` stays.
 * A zone set from the device's own row or a group's device pick is the
 * device's reported name, which the form writes in the spelling the device can
 * use (117a, `writtenZone`).
 */
export type ZonePick = { kind: 'zone'; zone: string | null } | { kind: 'unchanged' };

/** One row of the picker. */
export interface ZonePickerRow {
  key: string;
  /** What the row says, a mark and its reason included. */
  text: string;
  /** Whether choosing it does anything; a marked row says why in `text`. */
  selectable: boolean;
  pick: ZonePick;
  /** Whether the row is the zone in effect; the phone marks it selected. */
  current: boolean;
}

/** The picker's rows for a query, and the count line under the search. */
export interface ZonePickerList {
  rows: ZonePickerRow[];
  count: string;
}

/** What the picker lays out. */
export interface ZonePickerQuestion {
  query: string;
  /** From `zoneGroups`, for the series' stored zone, the device and the calendar. */
  groups: ZoneGroups;
  /** From `zoneOffsets`, at the series' start. */
  offsets: ZoneOffsets;
  regionNames: RegionName[];
  /** The zone the series stores, as stored; `null` for none. */
  stored: string | null;
  /** The device's zone name as the runtime reports it (`deviceZoneName`). */
  device: string | null;
  t: Translate;
  /** Writes a `YYYY-MM-DD` day in this surface's language. */
  formatDay: (day: string) => string;
}

const UNCHANGED: ZonePick = { kind: 'unchanged' };

/** The rows for a query: above the list, while the search is empty, the
 *  zone in effect, the device's zone and UTC; then the hits in list order. */
export function zonePickerList(q: ZonePickerQuestion): ZonePickerList {
  const { t } = q;
  const labels = listedZoneLabels();
  const answer = searchZones(q.query, q.regionNames, q.offsets, q.groups);
  const placed = zoneChoice(q.stored, q.device);
  const position = (zone: string) => {
    const at = labels.findIndex((label) => label.zone === zone);
    return at < 0 ? null : at;
  };
  const storedAt =
    placed.stored.kind === 'listed'
      ? placed.stored.position
      : placed.stored.kind === 'not_listed'
        ? position(placed.stored.canonical)
        : null;
  const onUtc = placed.stored.kind === 'utc';
  const deviceAt = placed.device?.kind === 'listed' ? placed.device.position : null;
  const sameAsStored = (name: string | null) =>
    name !== null && q.stored !== null && name.toLowerCase() === q.stored.toLowerCase();

  const zonePick = (at: number): ZonePick =>
    at === storedAt
      ? UNCHANGED
      : { kind: 'zone', zone: at === deviceAt && q.device !== null ? q.device : labels[at].zone };
  const utcPick: ZonePick = onUtc ? UNCHANGED : { kind: 'zone', zone: null };
  const cityEntry = (at: number) => zoneEntryText(labels[at], q.offsets.zones[at].offset, t);
  const marked = (text: string, refusal: ZoneRefusal | null, group: boolean) =>
    refusal === null
      ? text
      : t(`dialogs.seriesTimeZone.${group ? 'markGroup' : 'mark'}.${refusal}`, { entry: text });

  const rows: ZonePickerRow[] = [];
  if (q.query.trim() === '') {
    for (const head of q.groups.head) {
      const row = headRow(head);
      if (row !== null) rows.push(row);
    }
  }
  for (const hit of answer.hits) {
    const entry = hit.entry;
    switch (entry.kind) {
      case 'utc':
        rows.push({ key: 'utc', text: zoneEntryText(null, null, t), selectable: true, pick: utcPick, current: onUtc });
        break;
      case 'group': {
        const group = q.groups.groups[entry.group];
        const pick = group.pick;
        rows.push({
          key: `group-${entry.group}`,
          text: marked(zoneGroupText(group, t, q.formatDay), group.refusal, true),
          selectable: pick !== null,
          pick:
            pick === null || pick.source === 'stored'
              ? UNCHANGED
              : pick.source === 'device' && q.device !== null
                ? { kind: 'zone', zone: q.device }
                : zonePick(pick.position),
          current: pick?.source === 'stored',
        });
        break;
      }
      case 'zone': {
        const refusal = q.groups.refused[entry.position];
        rows.push({
          key: `zone-${entry.position}`,
          text: marked(zoneHitText(hit, q.offsets, t, q.groups, q.formatDay), refusal, false),
          selectable: refusal === null,
          pick: zonePick(entry.position),
          current: entry.position === storedAt,
        });
        break;
      }
    }
  }
  return { rows, count: zoneCountText(answer.hits.length, q.query.trim(), t, answer) };

  function headRow(head: HeadEntry): ZonePickerRow | null {
    const choice = head.choice;
    switch (head.kind) {
      case 'current': {
        const entry =
          choice.kind === 'listed'
            ? cityEntry(choice.position)
            : choice.kind === 'not_listed'
              ? t('dialogs.seriesTimeZone.notListed', { name: choice.name })
              : choice.kind === 'unreadable'
                ? t('dialogs.seriesTimeZone.unreadable', { name: choice.name })
                : null;
        if (entry === null) return null;
        return {
          key: 'head-current',
          text: t('dialogs.seriesTimeZone.headCurrent', { entry }),
          selectable: true,
          pick: UNCHANGED,
          current: true,
        };
      }
      case 'device': {
        const entry =
          choice.kind === 'listed'
            ? cityEntry(choice.position)
            : choice.kind === 'not_listed'
              ? t('dialogs.seriesTimeZone.notListed', { name: choice.name })
              : null;
        if (entry === null) return null;
        const here = choice.kind === 'listed' && choice.position === storedAt;
        const unchanged = here || sameAsStored(q.device);
        return {
          key: 'head-device',
          text: marked(t('dialogs.seriesTimeZone.headDevice', { entry }), head.refusal, false),
          selectable: head.refusal === null,
          pick: unchanged ? UNCHANGED : { kind: 'zone', zone: q.device },
          current: unchanged,
        };
      }
      case 'utc':
        return { key: 'head-utc', text: zoneEntryText(null, null, t), selectable: true, pick: utcPick, current: onUtc };
    }
  }
}
