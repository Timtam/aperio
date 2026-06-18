import type { CalendarEvent } from '../api/types';
import { localDateKey } from './dateKey';

// The pure day-spreading helpers (daysCoveredKeys / multiDayInfo /
// eventCoversDay / expandToDayOccurrences) moved into @aperio/shared so the
// mobile Agenda reuses them — re-exported here so the desktop's `./multiDay`
// import path is unchanged. The grid-only all-day bar packing (Week/Month)
// stays desktop-local below.
export {
  daysCoveredKeys,
  multiDayInfo,
  eventCoversDay,
  expandToDayOccurrences,
} from '@aperio/shared';
export type { MultiDayInfo } from '@aperio/shared';

import { daysCoveredKeys } from '@aperio/shared';
import type { DayOccurrence as SharedDayOccurrence } from '@aperio/shared';

/** Desktop alias: an agenda day-occurrence always carries a full
 *  `CalendarEvent` (the shared type is generic over the event shape). */
export type DayOccurrence = SharedDayOccurrence<CalendarEvent>;

/**
 * One bar in an all-day lane: an all-day event positioned on a single
 * contiguous run of days, packed into a vertical lane to avoid overlap.
 *
 * Used by WeekView (one lane row above the week's day cells) and MonthView
 * (one lane row per week-row of the month grid). Both surfaces feed a 7-day
 * window into `buildAllDayBars` and consume the same bar geometry. This stays
 * desktop-only — a linear screen-reader-first agenda needs no lane packing.
 */
export interface AllDayBar {
  event: CalendarEvent;
  /** Inclusive 1-based grid column for the leading day in this row. */
  startCol: number;
  /** Inclusive 1-based grid column for the trailing day in this row. */
  endCol: number;
  /** 0-based lane (vertical stack slot) inside the lane container. */
  lane: number;
  /** True if the event began before the visible window — the bar should
   *  visually flow off the leading edge (flat corner, optional ‹ glyph). */
  continuesBefore: boolean;
  /** True if the event extends past the visible window. */
  continuesAfter: boolean;
}

/**
 * Pack all-day events into bars for one contiguous run of days.
 *
 * `days` is the visible window (7 days for a WeekView week; 7 days for one row
 * of MonthView). Each event whose covered keys intersect the window gets one
 * `AllDayBar` clipped to the window's edges.
 *
 * Lane assignment is greedy first-fit: bars sorted by start column, then by
 * length descending, are placed into the first lane whose trailing column
 * doesn't extend into the bar's start.
 *
 * Returns an empty array when there's nothing to render so the lane container
 * can skip drawing itself entirely.
 */
export function buildAllDayBars(
  events: CalendarEvent[],
  days: Date[],
): AllDayBar[] {
  if (days.length === 0) return [];
  const windowStart = localDateKey(days[0]);
  const windowEnd = localDateKey(days[days.length - 1]);

  // Map keys → 0-based column for O(1) lookup.
  const colByKey = new Map<string, number>();
  days.forEach((d, i) => colByKey.set(localDateKey(d), i));

  // Pre-compute bars without lanes.
  const pending: Omit<AllDayBar, 'lane'>[] = [];
  events.forEach((ev) => {
    if (!ev.all_day) return;
    const keys = daysCoveredKeys(ev);
    if (keys.length === 0) return;
    const firstKey = keys[0];
    const lastKey = keys[keys.length - 1];
    // Skip events entirely outside the window.
    if (lastKey < windowStart || firstKey > windowEnd) return;

    const clippedFirst = firstKey < windowStart ? windowStart : firstKey;
    const clippedLast = lastKey > windowEnd ? windowEnd : lastKey;
    const startIdx = colByKey.get(clippedFirst);
    const endIdx = colByKey.get(clippedLast);
    if (startIdx === undefined || endIdx === undefined) return;

    pending.push({
      event: ev,
      startCol: startIdx + 1,
      endCol: endIdx + 1,
      continuesBefore: firstKey < windowStart,
      continuesAfter: lastKey > windowEnd,
    });
  });

  // Stable order before lane packing: leftmost first, then longest first so a
  // Mon–Sun bar lands in lane 0 and shorter overlapping bars stack above it.
  pending.sort((a, b) => {
    if (a.startCol !== b.startCol) return a.startCol - b.startCol;
    return b.endCol - b.startCol - (a.endCol - a.startCol);
  });

  const laneTrailingCol: number[] = [];
  return pending.map((bar) => {
    let lane = 0;
    while (
      lane < laneTrailingCol.length &&
      laneTrailingCol[lane] >= bar.startCol
    ) {
      lane++;
    }
    laneTrailingCol[lane] = bar.endCol;
    return { ...bar, lane };
  });
}
