import { addDays, differenceInDays } from 'date-fns';

import type { CalendarEvent } from '../api/types';
import { localDateKey } from './dateKey';

/**
 * Day-spreading helpers for events that cover more than one calendar
 * day (typically multi-day all-day events: a two-week vacation, a
 * conference that runs Mon–Fri).
 *
 * Without help, the views only see an event on its start day — they
 * group by `localDateKey(ev.start)` and filter with `isSameDay(ev.start,
 * day)`. Day 3 of a 14-day vacation would silently disappear from the
 * calendar, even though it's still going on. These helpers let each
 * view treat the event as present on every day it covers.
 *
 * Scope: only **all-day** events are spread. Timed events that happen
 * to cross midnight (an 11 p.m. → 1 a.m. meeting) stay anchored to
 * their start day — that's how Outlook / Google handle it too, and
 * splitting them visually needs a different model (two half-bars on
 * adjacent days).
 *
 * Timezone handling: all-day events come in from CalDAV / our local
 * store as UTC-midnight (`DTSTART;VALUE=DATE:20260520` → 00:00 UTC on
 * 2026-05-20). We anchor both endpoints at the user's local midnight
 * before iterating so DST transitions in the middle of the range
 * don't drop or duplicate a day. Users in timezones west of UTC can
 * still see the day shifted by one in extreme cases — that's a
 * pre-existing rendering issue with all-day storage and out of scope
 * here.
 */

/**
 * Return every local-date key the event is visible on, in order.
 *
 * - All-day, span > 0: walks [start, end) and emits one key per day.
 * - All-day, span 0 (DTEND missing or == DTSTART): emits the start day.
 * - Timed: emits only the start day's key.
 */
export function daysCoveredKeys(ev: CalendarEvent): string[] {
  if (!ev.all_day) {
    return [localDateKey(new Date(ev.start))];
  }
  const start = new Date(ev.start);
  const end = new Date(ev.end);
  // Anchor at local midnight before counting so a DST transition in
  // the middle of the range doesn't yield a fractional day.
  const startMid = new Date(
    start.getFullYear(),
    start.getMonth(),
    start.getDate(),
  );
  const endMid = new Date(end.getFullYear(), end.getMonth(), end.getDate());
  const span = Math.max(1, differenceInDays(endMid, startMid));
  const keys: string[] = [];
  for (let i = 0; i < span; i++) {
    keys.push(localDateKey(addDays(startMid, i)));
  }
  return keys;
}

export interface MultiDayInfo {
  /** 1-based position of `day` inside the span. */
  dayIndex: number;
  /** Total length of the span, in days. */
  totalDays: number;
}

/**
 * Return position info for a multi-day event on a given day, or `null`
 * if the event spans only one day (so the views can skip the "(3/14)"
 * suffix on single-day events).
 */
export function multiDayInfo(
  ev: CalendarEvent,
  day: Date,
): MultiDayInfo | null {
  if (!ev.all_day) return null;
  const keys = daysCoveredKeys(ev);
  if (keys.length <= 1) return null;
  const dayKey = localDateKey(day);
  const idx = keys.indexOf(dayKey);
  if (idx < 0) return null;
  return { dayIndex: idx + 1, totalDays: keys.length };
}

/**
 * Does this event cover the given day?
 *
 * Convenience for views that filter rather than group (DayView,
 * AgendaView's per-day section header).
 */
export function eventCoversDay(ev: CalendarEvent, day: Date): boolean {
  return daysCoveredKeys(ev).includes(localDateKey(day));
}

/**
 * One renderable row for the agenda. Multi-day all-day events become
 * several occurrences — one per day they're visible on — each carrying
 * the source event plus the position info that drives the "(3/14)"
 * suffix.
 */
export interface DayOccurrence {
  ev: CalendarEvent;
  /** The local day this occurrence represents. */
  day: Date;
  span: MultiDayInfo | null;
}

/**
 * Expand each event into one DayOccurrence per covered day, clipping
 * to the visible range and sorting by (day, original start time).
 *
 * Use this when the surface is a flat chronological list (AgendaView).
 * Grid views can stick with `daysCoveredKeys` for bucketing — that
 * keeps the per-cell rendering path unchanged.
 *
 * Range is interpreted as [start, end) on the calendar day axis. An
 * event whose covered days fall partly outside the range gets clipped
 * to the inside; events that fall entirely outside contribute nothing.
 */
export function expandToDayOccurrences(
  events: CalendarEvent[],
  range: { start: Date; end: Date },
): DayOccurrence[] {
  const out: DayOccurrence[] = [];
  // Inclusive day bounds for the local-date comparison.
  const startKey = localDateKey(range.start);
  const endKey = localDateKey(range.end);

  events.forEach((ev) => {
    if (ev.all_day) {
      const keys = daysCoveredKeys(ev);
      const total = keys.length;
      keys.forEach((k, idx) => {
        if (k < startKey || k > endKey) return;
        // Reconstruct a local-midnight Date from the key for stable
        // sorting. JS's Date parser is locale-sensitive on YYYY-MM-DD
        // (UTC interpretation), so we go through the components.
        const [y, m, d] = k.split('-').map(Number);
        const day = new Date(y, m - 1, d);
        out.push({
          ev,
          day,
          span: total > 1 ? { dayIndex: idx + 1, totalDays: total } : null,
        });
      });
    } else {
      // Timed event: anchored to its start day.
      const start = new Date(ev.start);
      const k = localDateKey(start);
      if (k < startKey || k > endKey) return;
      const [y, m, d] = k.split('-').map(Number);
      out.push({ ev, day: new Date(y, m - 1, d), span: null });
    }
  });

  out.sort((a, b) => {
    const dDay = a.day.getTime() - b.day.getTime();
    if (dDay !== 0) return dDay;
    return (
      new Date(a.ev.start).getTime() - new Date(b.ev.start).getTime()
    );
  });
  return out;
}

/**
 * One bar in an all-day lane: an all-day event positioned on a single
 * contiguous run of days, packed into a vertical lane to avoid
 * overlap.
 *
 * Used by WeekView (one lane row above the week's day cells) and
 * MonthView (one lane row per week-row of the month grid). Both
 * surfaces feed a 7-day window into `buildAllDayBars` and consume
 * the same bar geometry.
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
 * `days` is the visible window (7 days for a WeekView week; 7 days for
 * one row of MonthView). Each event whose covered keys intersect the
 * window gets one `AllDayBar` clipped to the window's edges.
 *
 * Lane assignment is greedy first-fit: bars sorted by start column,
 * then by length descending, are placed into the first lane whose
 * trailing column doesn't extend into the bar's start. With three
 * concurrent vacations on Mon–Wed you get lanes 0, 1, 2 — exactly the
 * same look as Outlook / Google's all-day strip.
 *
 * Returns an empty array when there's nothing to render so the lane
 * container can skip drawing itself entirely.
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

  // Stable order before lane packing: leftmost first, then longest first
  // so a Mon–Sun bar lands in lane 0 and shorter overlapping bars stack
  // above it.
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
