/**
 * Pure date/time helpers and "smart picker" logic for the event form.
 *
 * Lives in its own module (rather than inside `EventDialog`) so the
 * duration-preserving and default-time behaviour can be unit-tested in
 * isolation, and so `EventDialog` keeps exporting only its component
 * (react-refresh friendly).
 *
 * Conventions match Outlook / Google Calendar:
 *  - editing the start slides the end along to preserve the duration;
 *  - editing the end only changes the duration (clamped so it can never
 *    fall before the start);
 *  - a new event defaults to the day the view is focused on, at a
 *    sensible time of day.
 */

/** The date/time subset of the event form the smart logic operates on. */
export interface EventTimes {
  startDate: string; // YYYY-MM-DD (local)
  startTime: string; // HH:MM (local)
  endDate: string; // YYYY-MM-DD (local)
  endTime: string; // HH:MM (local)
  allDay: boolean;
}

/** Format a local Date as `YYYY-MM-DD` for `<input type="date">`.
 *  Built from local components rather than `toISOString()` (which uses
 *  UTC and can shift the day in timezones east of GMT). */
export function dateInput(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

/** Format a local Date as `HH:MM` for `<input type="time">`. */
export function timeInput(d: Date): string {
  const h = String(d.getHours()).padStart(2, '0');
  const m = String(d.getMinutes()).padStart(2, '0');
  return `${h}:${m}`;
}

/** Parse a `YYYY-MM-DD` or full ISO string into a Date at the start of
 *  the local day. Returns null when the input is undefined / invalid. */
export function parseDefaultDate(input: string | undefined): Date | null {
  if (!input) return null;
  const isoDay = input.length >= 10 ? input.slice(0, 10) : input;
  const [y, m, d] = isoDay.split('-').map(Number);
  if (!y || !m || !d) return null;
  const date = new Date(y, m - 1, d, 0, 0, 0);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Combine a `YYYY-MM-DD` + `HH:MM` into a local Date. For all-day the
 *  time component is ignored and midnight is used, so day-granular
 *  duration maths stay correct. Returns null on a malformed value. */
export function combine(
  date: string,
  time: string,
  allDay: boolean,
): Date | null {
  const [y, m, d] = date.split('-').map(Number);
  if (!y || !m || !d) return null;
  let hh = 0;
  let mm = 0;
  if (!allDay) {
    const parts = time.split(':').map(Number);
    if (parts.length < 2 || parts.some(Number.isNaN)) return null;
    [hh, mm] = parts;
  }
  const out = new Date(y, m - 1, d, hh, mm, 0, 0);
  return Number.isNaN(out.getTime()) ? null : out;
}

/** Combine a `YYYY-MM-DD` + `HH:MM` into an ISO 8601 wire string.
 *  Returns null when the date (or, for timed events, the time) is
 *  unparseable. */
export function toIso(
  date: string,
  time: string,
  allDay: boolean,
): string | null {
  const d = combine(date, time, allDay);
  return d ? d.toISOString() : null;
}

/** Parse the form's `YYYY-MM-DD` start string into a local-midnight Date
 *  for the recurrence picker. Falls back to today on an empty/garbled
 *  value. */
export function recurrenceStartDate(date: string): Date {
  const [y, m, d] = date.split('-').map(Number);
  if (!y || !m || !d) return new Date();
  return new Date(y, m - 1, d);
}

/** Whether two Dates fall on the same local calendar day. */
function sameLocalDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/** Round `now` up to the next :00 / :30 slot, always landing strictly in
 *  the future (14:00 → 14:30, 14:23 → 14:30, 14:30 → 15:00). */
export function nextHalfHour(now: Date): Date {
  const d = new Date(now);
  d.setSeconds(0, 0);
  // setMinutes(60) rolls over into the next hour at :00.
  d.setMinutes(d.getMinutes() < 30 ? 30 : 60);
  return d;
}

/**
 * Pick the default start/end for a *new* event, Outlook/Google-style:
 *  - anchored on today  → next :00/:30 slot (so it's just ahead of now);
 *  - anchored on another day → 09:00 (a sensible start of the workday);
 *  - no anchor at all   → next full hour from now (legacy fallback).
 * The slot is one hour long.
 */
export function defaultNewEventTimes(
  defaultDate: string | undefined,
  now: Date,
): { start: Date; end: Date } {
  const anchoredDay = parseDefaultDate(defaultDate);
  let start: Date;
  if (anchoredDay) {
    if (sameLocalDay(anchoredDay, now)) {
      start = nextHalfHour(now);
    } else {
      start = new Date(anchoredDay);
      start.setHours(9, 0, 0, 0);
    }
  } else {
    start = new Date(now);
    start.setMinutes(0, 0, 0);
    start.setHours(start.getHours() + 1);
  }
  const end = new Date(start);
  end.setHours(end.getHours() + 1);
  return { start, end };
}

/**
 * Apply a single date/time field change with duration-preserving smarts:
 *
 *  - changing the **start** date or time slides the **end** by the same
 *    delta, so the duration (and any multi-day span) is preserved;
 *  - changing the **end** only resizes the event, but is clamped so the
 *    end can never fall before the start;
 *
 * `allDay` is handled by `combine()` collapsing the time to midnight, so
 * an all-day start-date change shifts the end by whole days. The time
 * strings themselves are never cleared — toggling all-day off restores
 * the previous times.
 *
 * Returns the full patched {@link EventTimes}; callers merge it back into
 * their form state.
 */
export function applyDateTimeChange(
  cur: EventTimes,
  key: 'startDate' | 'startTime' | 'endDate' | 'endTime',
  value: string,
): EventTimes {
  const next: EventTimes = { ...cur, [key]: value };

  if (key === 'startDate' || key === 'startTime') {
    const oldStart = combine(cur.startDate, cur.startTime, cur.allDay);
    const newStart = combine(next.startDate, next.startTime, next.allDay);
    const oldEnd = combine(cur.endDate, cur.endTime, cur.allDay);
    if (oldStart && newStart && oldEnd) {
      const deltaMs = newStart.getTime() - oldStart.getTime();
      const newEnd = new Date(oldEnd.getTime() + deltaMs);
      next.endDate = dateInput(newEnd);
      // Keep the wall-clock end time for timed events; for all-day the
      // time is meaningless so we leave the stored string untouched.
      if (!cur.allDay) next.endTime = timeInput(newEnd);
    }
    return next;
  }

  // End edit: only resize, but never let the end precede the start.
  const start = combine(next.startDate, next.startTime, next.allDay);
  const end = combine(next.endDate, next.endTime, next.allDay);
  if (start && end && end.getTime() < start.getTime()) {
    next.endDate = next.startDate;
    next.endTime = next.startTime;
  }
  return next;
}
