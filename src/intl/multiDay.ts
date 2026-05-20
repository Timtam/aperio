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
