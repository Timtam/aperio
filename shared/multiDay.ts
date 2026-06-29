import { localDateKey } from './dateKey';

/**
 * Day-spreading helpers for events that cover more than one calendar day
 * (typically multi-day all-day events: a two-week vacation, a conference that
 * runs Mon–Fri). Shared by the desktop calendar views and the mobile Agenda.
 *
 * Without help, the views only see an event on its start day — they group by
 * `localDateKey(ev.start)`. Day 3 of a 14-day vacation would silently disappear
 * from the calendar. These helpers let each view treat the event as present on
 * every day it covers.
 *
 * Scope: all-day events are spread across [start, end). TIMED events that cross
 * midnight (an 11 p.m. → 1 a.m. meeting) are also spread onto each day they
 * actually occupy, so the post-midnight tail shows on the next day too; the
 * per-day `eventSpanForDay` clamp then renders each day's portion (day N:
 * 23:00–24:00, day N+1: 00:00–01:00). An event ending exactly at midnight does
 * NOT leak into the next day (end-exclusive).
 *
 * Timezone handling: all-day events are anchored at the user's LOCAL midnight,
 * expressed as a UTC instant — the convention the CalDAV adapter's
 * `naive_date_to_utc` writes (`DTSTART;VALUE=DATE:20260520` → local-midnight-as-
 * UTC, deliberately NOT 00:00 UTC, which would render a day early west of UTC).
 * So we read the local calendar day straight back off the instant with local-time
 * accessors and walk by whole calendar days (`setDate(+1)`); a DST transition in
 * the middle of the range never drops or duplicates a day. (This replaces the
 * desktop's old date-fns differenceInDays/addDays — the walk is DST-safe and
 * dependency-free, so the module runs on mobile too.)
 */

/** The minimal event shape the day-spreaders need (CalendarEvent satisfies it
 *  on both desktop and mobile). Generic callers keep their full event type. */
export interface DaySpanEventLike {
  /** RFC-3339 / ISO start instant. */
  start: string;
  /** RFC-3339 / ISO end instant (DTEND — exclusive for all-day events). */
  end: string;
  all_day: boolean;
}

/**
 * Return every local-date key the event is visible on, in order.
 *
 * - All-day, span > 0: walks [start, end) and emits one key per day.
 * - All-day, span 0 (DTEND missing or == DTSTART): emits the start day.
 * - Timed, same day: emits the start day's key.
 * - Timed crossing midnight: emits each day whose local midnight falls strictly
 *   before the (exclusive) end instant (so an 11pm→1am meeting emits both days).
 */
export function daysCoveredKeys(ev: DaySpanEventLike): string[] {
  const start = new Date(ev.start);
  const end = new Date(ev.end);
  // Anchor at local midnight before walking so a DST transition in the middle
  // of the range doesn't yield a fractional day. setDate(+1) is DST-safe — it
  // moves by a calendar day, not 24 fixed hours.
  const startMid = new Date(start.getFullYear(), start.getMonth(), start.getDate());
  const keys = [localDateKey(startMid)];
  const cursor = new Date(startMid);
  if (ev.all_day) {
    // DTEND is an EXCLUSIVE date: include each day strictly before the end day.
    const endMid = new Date(end.getFullYear(), end.getMonth(), end.getDate());
    const endKey = localDateKey(endMid);
    for (;;) {
      cursor.setDate(cursor.getDate() + 1);
      const k = localDateKey(cursor);
      if (k >= endKey) break;
      keys.push(k);
    }
    return keys;
  }
  // Timed: include each later day whose local midnight is strictly before the
  // (exclusive) end instant. The guard keeps a bad/zero/short span (incl. a
  // NaN end) on the start day only — and prevents an unbounded walk.
  if (!(end.getTime() > startMid.getTime())) return keys;
  for (;;) {
    cursor.setDate(cursor.getDate() + 1);
    if (cursor.getTime() >= end.getTime()) break;
    keys.push(localDateKey(cursor));
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
 * Return position info for a multi-day event on a given day, or `null` if the
 * event spans only one day (so views can skip the "(3/14)" suffix). Works for
 * all-day spans AND timed events that cross midnight (a 23:00→01:00 meeting is
 * "1/2" on its start day and "2/2" the next).
 */
export function multiDayInfo(ev: DaySpanEventLike, day: Date): MultiDayInfo | null {
  const keys = daysCoveredKeys(ev);
  if (keys.length <= 1) return null;
  const idx = keys.indexOf(localDateKey(day));
  if (idx < 0) return null;
  return { dayIndex: idx + 1, totalDays: keys.length };
}

/** Does this event cover the given day? Convenience for filter-style views. */
export function eventCoversDay(ev: DaySpanEventLike, day: Date): boolean {
  return daysCoveredKeys(ev).includes(localDateKey(day));
}

/**
 * One renderable row for the agenda. Multi-day all-day events become several
 * occurrences — one per day they're visible on — each carrying the source event
 * plus the position info that drives the "(3/14)" suffix.
 */
export interface DayOccurrence<E extends DaySpanEventLike> {
  ev: E;
  /** The local day this occurrence represents. */
  day: Date;
  span: MultiDayInfo | null;
}

/**
 * Expand each event into one DayOccurrence per covered day, clipping to the
 * visible range (inclusive on both day bounds) and sorting by (day, original
 * start time). Use this when the surface is a flat chronological list (Agenda).
 */
export function expandToDayOccurrences<E extends DaySpanEventLike>(
  events: E[],
  range: { start: Date; end: Date },
): DayOccurrence<E>[] {
  const out: DayOccurrence<E>[] = [];
  const startKey = localDateKey(range.start);
  const endKey = localDateKey(range.end);

  events.forEach((ev) => {
    if (ev.all_day) {
      const keys = daysCoveredKeys(ev);
      const total = keys.length;
      keys.forEach((k, idx) => {
        if (k < startKey || k > endKey) return;
        // Reconstruct a local-midnight Date from the key for stable sorting.
        // JS's Date parser treats bare YYYY-MM-DD as UTC, so go via components.
        const [y, m, d] = k.split('-').map(Number);
        out.push({
          ev,
          day: new Date(y, m - 1, d),
          span: total > 1 ? { dayIndex: idx + 1, totalDays: total } : null,
        });
      });
    } else {
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
    return new Date(a.ev.start).getTime() - new Date(b.ev.start).getTime();
  });
  return out;
}
