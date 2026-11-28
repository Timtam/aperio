// A sync conflict on a series' repeat that differs only in the series' zone
// (stage 14 of DESIGN-series-time-zone.md). The conflict dialog shows a
// field's two values as stored, which for a repeat is a block of JSON a
// screen reader reads character by character, while the one thing that
// differs is the zone. Such a conflict is named as the series' time zone, by
// city, on both surfaces and in the copied text.

import { zoneCityName } from './zoneFormText';

/** The subset of a conflict record this module needs; the platforms' full
 *  `SyncConflict` types satisfy it. */
export interface ZoneConflictLike {
  row_kind: string;
  field: string;
  local_value: string | null;
  remote_value: string | null;
}

/** A repeat as the conflict stores it: JSON with a rule and maybe a zone. */
type StoredRepeat = Record<string, unknown> & { rrule: string; tzid?: string | null };

function storedRepeat(raw: string | null): StoredRepeat | null {
  if (raw === null) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (value === null || typeof value !== 'object' || Array.isArray(value)) return null;
    return typeof (value as { rrule?: unknown }).rrule === 'string' ? (value as StoredRepeat) : null;
  } catch {
    return null;
  }
}

/** The value as text with its keys sorted and its empty parts left out, so
 *  two spellings of one repeat read the same: a key one side writes as
 *  `null` or `[]` and the other leaves out says nothing either way. */
function canonicalText(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalText).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>)
      .filter(([, part]) => part !== null && part !== undefined && !(Array.isArray(part) && part.length === 0))
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return `{${entries.map(([key, part]) => `${JSON.stringify(key)}:${canonicalText(part)}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

/**
 * The zone on each side, by city ("Berlin", "New York", "UTC"), where a
 * conflict on an event's repeat differs in the series' zone and in nothing
 * else; `null` for any other conflict, which shows its values as they are.
 * Two names of one clock (UTC and none, an alias and its zone) are no
 * difference: the sync engine no longer raises such a conflict.
 */
export function seriesZoneConflict(
  conflict: ZoneConflictLike,
): { local: string; remote: string } | null {
  if (conflict.row_kind !== 'event' || conflict.field !== 'recurrence') return null;
  const local = storedRepeat(conflict.local_value);
  const remote = storedRepeat(conflict.remote_value);
  if (local === null || remote === null) return null;
  const { tzid: localZone, ...localRest } = local;
  const { tzid: remoteZone, ...remoteRest } = remote;
  if (canonicalText(localRest) !== canonicalText(remoteRest)) return null;
  const localName = zoneCityName(localZone ?? null);
  const remoteName = zoneCityName(remoteZone ?? null);
  return localName === remoteName ? null : { local: localName, remote: remoteName };
}
