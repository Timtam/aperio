// Day markers — the vocabulary of what a day was like, and one record per day.
//
// Local-only and always synced: no external provider models "how was Tuesday",
// and this is the most private thing in the app. JSON passthrough over the
// Host's day-marker methods, same as ./colorLabels next door.

import type { DayLog, DayMarker } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import { notifyDayMarkersChanged } from '../state/dayMarkersChanged';

export type { DayLog, DayMarker };

// ── The vocabulary ───────────────────────────────────────────────────────────

export const listDayMarkers = async (): Promise<DayMarker[]> =>
  JSON.parse(await CalFfi.listDayMarkersJson()) as DayMarker[];

export const createDayMarker = async (
  name: string,
  symbol: string | null,
  colorLabel: string | null,
): Promise<DayMarker> =>
  announceWrite(
    JSON.parse(
      await CalFfi.createDayMarkerJson(name, symbol, colorLabel),
    ) as DayMarker,
  );

/** Write a marker back whole — rename, re-symbol, recolour and reorder are all
 *  this one call, so the screens need one code path rather than four. */
export const updateDayMarker = async (marker: DayMarker): Promise<DayMarker> =>
  announceWrite(
    JSON.parse(
      await CalFfi.updateDayMarkerJson(JSON.stringify(marker)),
    ) as DayMarker,
  );

/** Remove a marker. Logged days keep their record — it simply stops
 *  resolving, which is how it disappears from history without a rewrite. */
export const deleteDayMarker = async (id: string): Promise<void> => {
  await CalFfi.deleteDayMarker(id);
  announceWrite(undefined);
};

// ── The per-day log ──────────────────────────────────────────────────────────

/** One day. An untouched day comes back as an empty log, never null. */
export const getDayLog = async (day: string): Promise<DayLog> =>
  JSON.parse(await CalFfi.dayLogJson(day)) as DayLog;

/** Every logged day in an inclusive range — one call per view, not per day. */
export const getDayLogsInRange = async (
  from: string,
  to: string,
): Promise<DayLog[]> =>
  JSON.parse(await CalFfi.dayLogsInRangeJson(from, to)) as DayLog[];

export const setDayLog = async (log: DayLog): Promise<DayLog> =>
  announceWrite(
    JSON.parse(await CalFfi.setDayLogJson(JSON.stringify(log))) as DayLog,
  );

/** Every day-marker write passes its result through here so no caller can
 *  forget to tell the other screens. Only on SUCCESS — a rejected write
 *  changed nothing, and asking everyone to re-read would make them repaint the
 *  state they already had. */
function announceWrite<T>(result: T): T {
  notifyDayMarkersChanged();
  return result;
}
