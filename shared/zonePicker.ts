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
  /** Whether the row says the zone in effect: above the list as the core says
   *  ("Aktuell" always, even where it is marked off, and the device's row
   *  standing in for it), in the list the rows that keep it. The phone marks
   *  it selected. */
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
  /** From `zoneGroups`, for the series' stored zone, the device, the calendar
   *  and the names this device refuses (`unusableZones`). */
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
  const onUtc = placed.stored.kind === 'utc';
  // The zone in effect is kept as it is spelled (120a), by the city and the
  // group the core names: only where the calendar stores it and this device
  // can use its stored spelling. A stored Europe/Kyiv on a device that refuses
  // it is kept by none, and its rows change it, as the core's group does (to
  // the device's Europe/Kiev).
  const keptAt = q.groups.kept;
  const deviceAt = placed.device?.kind === 'listed' ? placed.device.position : null;

  const zonePick = (at: number): ZonePick =>
    at === keptAt
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
        rows.push({
          key: 'utc',
          text: zoneHitText(hit, q.offsets, t, q.groups, q.formatDay),
          selectable: true,
          pick: utcPick,
          current: onUtc,
        });
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
        // The zone in effect, kept in a spelling this device can use, is
        // no refusal, though the device refuses its listed name.
        const kept = entry.position === keptAt;
        const refusal = kept ? null : q.groups.refused[entry.position];
        rows.push({
          key: `zone-${entry.position}`,
          text: marked(zoneHitText(hit, q.offsets, t, q.groups, q.formatDay), refusal, false),
          selectable: refusal === null,
          pick: zonePick(entry.position),
          current: kept,
        });
        break;
      }
    }
  }
  return { rows, count: zoneCountText(answer.hits.length, q.query.trim(), t, answer) };

  // The core says what each row above the list is: its mark, whether it says
  // the zone in effect, and whether choosing it keeps that zone. "Aktuell" is
  // marked where the calendar cannot store the zone or this device refuses
  // its spelling, and another zone is the way out; a zone Aperio cannot read
  // says its own reason, and keeping it stays a choice.
  function headRow(head: HeadEntry): ZonePickerRow | null {
    const choice = head.choice;
    const row = (text: string, pick: ZonePick): ZonePickerRow => ({
      key: `head-${head.kind}`,
      text: marked(text, head.refusal, false),
      selectable: head.refusal === null,
      pick: head.keeps ? UNCHANGED : pick,
      current: head.current,
    });
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
        return entry === null ? null : row(t('dialogs.seriesTimeZone.headCurrent', { entry }), UNCHANGED);
      }
      case 'device': {
        const entry =
          choice.kind === 'listed'
            ? cityEntry(choice.position)
            : choice.kind === 'not_listed'
              ? t('dialogs.seriesTimeZone.notListed', { name: choice.name })
              : null;
        return entry === null
          ? null
          : row(t('dialogs.seriesTimeZone.headDevice', { entry }), { kind: 'zone', zone: q.device });
      }
      case 'utc':
        return row(zoneEntryText(null, null, t), { kind: 'zone', zone: null });
    }
  }
}
