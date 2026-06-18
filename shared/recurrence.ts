import { RRule, rrulestr } from 'rrule';

// Event recurrence expansion, shared by desktop + mobile. Generic over a
// minimal `RecurringEventLike` so it needs neither side's full `CalendarEvent`
// type (those still live per-app for now); any event with id/start/end + the
// `{rrule, exceptions}` recurrence shape works. Desktop re-exports this from
// `src/intl/recurrence.ts`; mobile imports it directly.

/** The minimal event shape the expander needs. Both the desktop and mobile
 *  `CalendarEvent` satisfy it. */
export interface RecurringEventLike {
  id: string;
  /** RFC-3339 (UTC) start/end of the master event. */
  start: string;
  end: string;
  recurrence: { rrule: string; exceptions: string[] } | null;
}

/** An expanded per-occurrence copy of `E`: same fields, but `start`/`end`
 *  shifted to the occurrence, a unique `id`, and the master id kept as
 *  `series_id` so the edit/delete layer can find the underlying row. */
export type ExpandedOccurrence<E extends RecurringEventLike> = E & {
  series_id: string;
  occurrence_start: string;
};

/**
 * Expand a recurring event into all of its occurrences inside `range`.
 *
 * Returns the original event unchanged when there is no recurrence rule.
 * Otherwise produces one copy per occurrence whose `start`/`end` match the
 * occurrence time and whose `id` is suffixed with the occurrence start (ISO) so
 * list keys stay unique; the master `id` is preserved as `series_id`. `EXDATE`
 * entries are honoured (rrule.js filters them out).
 *
 * Time-zone caveat: `start` is RFC-3339 (UTC). rrule.js works in `Date`
 * instants taken from `dtstart`; the result instants are re-serialised via
 * `toISOString()`.
 */
export function expandEvent<E extends RecurringEventLike>(
  event: E,
  range: { start: Date; end: Date },
): (E | ExpandedOccurrence<E>)[] {
  if (!event.recurrence?.rrule) {
    return [event];
  }

  const dtstart = new Date(event.start);
  const dtend = new Date(event.end);
  const duration = dtend.getTime() - dtstart.getTime();

  let rule: RRule;
  try {
    rule = buildRule(event.recurrence.rrule, dtstart);
  } catch (err) {
    // Bad rule string — fall back to showing the master at its stored start so
    // the user can still see and edit it.
    // eslint-disable-next-line no-console
    console.warn('failed to parse RRULE', event.recurrence.rrule, err);
    return [event];
  }

  // `between` is start-exclusive by default; `inc = true` makes the range
  // boundaries inclusive so an event starting exactly on a boundary appears.
  const occurrences = rule.between(range.start, range.end, true);
  if (occurrences.length === 0) {
    return [];
  }

  const exceptions = new Set(
    event.recurrence.exceptions.map((iso) => new Date(iso).getTime()),
  );

  return occurrences
    .filter((d) => !exceptions.has(d.getTime()))
    .map<ExpandedOccurrence<E>>((occStart) => {
      const occEnd = new Date(occStart.getTime() + duration);
      return {
        ...event,
        id: `${event.id}@${occStart.toISOString()}`,
        series_id: event.id,
        occurrence_start: occStart.toISOString(),
        start: occStart.toISOString(),
        end: occEnd.toISOString(),
      };
    });
}

function buildRule(rruleBody: string, dtstart: Date): RRule {
  // rrulestr accepts a full RFC-5545 "RRULE:..." block; if the stored string is
  // just the body (FREQ=...;BYDAY=...) prepend the marker.
  const body = rruleBody.trim();
  const text = body.toUpperCase().startsWith('RRULE:') ? body : `RRULE:${body}`;
  return rrulestr(text, { dtstart }) as RRule;
}

/**
 * Walk events through {@link expandEvent}, flatten, and sort chronologically.
 * The result is `E[]` (occurrences are assignment-compatible with `E`); callers
 * that need the underlying series read `series_id` via {@link seriesIdOf}.
 */
export function expandAll<E extends RecurringEventLike>(
  events: E[],
  range: { start: Date; end: Date },
): E[] {
  const out = events.flatMap((ev) => expandEvent(ev, range));
  out.sort((a, b) => a.start.localeCompare(b.start));
  return out;
}

/** Type guard: a synthetic occurrence vs a regular/master event. */
export function isExpandedOccurrence<E extends RecurringEventLike>(
  event: E,
): event is ExpandedOccurrence<E> {
  return (
    'series_id' in event &&
    typeof (event as ExpandedOccurrence<E>).series_id === 'string'
  );
}

/**
 * Underlying series id for an event row: the master's `series_id` for an
 * expanded occurrence, else `event.id`. Keying off `series_id` (not
 * `id.split('@')[0]`) is the canonical fix — Aperio CalDAV UIDs themselves
 * contain `@aperio`, so the split shortcut dropped half the master UID.
 */
export function seriesIdOf<E extends RecurringEventLike>(event: E): string {
  return isExpandedOccurrence(event) ? event.series_id : event.id;
}

/**
 * Occurrence-start ISO for an expanded occurrence, else `null` for a master.
 * Drives "delete only this occurrence" (append onto the master's EXDATE).
 */
export function occurrenceIsoOf<E extends RecurringEventLike>(
  event: E,
): string | null {
  return isExpandedOccurrence(event) ? event.occurrence_start : null;
}
