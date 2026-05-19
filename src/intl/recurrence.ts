import { RRule, rrulestr } from 'rrule';

import type { CalendarEvent, EventRecurrence } from '../api/types';

/**
 * Expand a recurring event into all of its occurrences inside `range`.
 *
 * Returns the original event unchanged when there is no recurrence
 * rule. Otherwise produces one `CalendarEvent` per occurrence whose
 * `start` and `end` match the occurrence time and whose `id` is suffixed
 * with the occurrence start in ISO form so React keys stay unique.
 * The original `id` is preserved as `series_id` on the synthetic events
 * so the dialog layer can locate the underlying row when the user edits
 * a recurring event.
 *
 * `EXDATE` entries from the recurrence model are honoured: rrule.js
 * filters them out of the occurrence list.
 *
 * Time-zone caveat: `event.start` is RFC 3339 (UTC). rrule.js works in
 * `Date` objects whose underlying instant is taken from `dtstart`. We
 * construct the dtstart from the UTC string and expand within the
 * supplied range — the result Date objects are UTC instants which we
 * re-serialise via toISOString().
 */
export interface ExpandedEvent extends CalendarEvent {
  series_id: string;
  occurrence_start: string;
}

export function expandEvent(
  event: CalendarEvent,
  range: { start: Date; end: Date },
): (CalendarEvent | ExpandedEvent)[] {
  if (!event.recurrence?.rrule) {
    return [event];
  }

  const dtstart = new Date(event.start);
  const dtend = new Date(event.end);
  const duration = dtend.getTime() - dtstart.getTime();

  let rule: RRule;
  try {
    rule = buildRule(event.recurrence, dtstart);
  } catch (err) {
    // Bad rule string — fall back to showing the master event at its
    // stored start so the user can still see and edit it.
    // eslint-disable-next-line no-console
    console.warn('failed to parse RRULE', event.recurrence.rrule, err);
    return [event];
  }

  // `between` is start-exclusive by default; pass `inc = true` so the
  // range boundaries are inclusive — events that start exactly on the
  // range boundary should still appear.
  const occurrences = rule.between(range.start, range.end, true);

  if (occurrences.length === 0) {
    return [];
  }

  const exceptions = new Set(
    event.recurrence.exceptions.map((iso) => new Date(iso).getTime()),
  );

  return occurrences
    .filter((d) => !exceptions.has(d.getTime()))
    .map<ExpandedEvent>((occStart) => {
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

function buildRule(recurrence: EventRecurrence, dtstart: Date): RRule {
  // rrulestr accepts a full RFC 5545 "RRULE:..." block. If the stored
  // string is just the rule body (FREQ=...;BYDAY=...) we prepend the
  // RRULE: marker so the parser is happy either way.
  const body = recurrence.rrule.trim();
  const text = body.toUpperCase().startsWith('RRULE:') ? body : `RRULE:${body}`;
  const rule = rrulestr(text, { dtstart }) as RRule;
  return rule;
}

/**
 * Convenience: walk an array of events and flat-map them through
 * `expandEvent`, then sort the result chronologically.
 *
 * Returns the same shape (`CalendarEvent[]`) since `ExpandedEvent` is
 * assignment-compatible with `CalendarEvent` — callers don't need to
 * special-case occurrences unless they want to find the underlying
 * series (in which case they read `series_id` off the augmented type).
 */
export function expandAll(
  events: CalendarEvent[],
  range: { start: Date; end: Date },
): CalendarEvent[] {
  const out = events.flatMap((ev) => expandEvent(ev, range));
  out.sort((a, b) => a.start.localeCompare(b.start));
  return out;
}

/** Type guard: distinguishes a synthetic occurrence from a regular event. */
export function isExpandedOccurrence(
  event: CalendarEvent,
): event is ExpandedEvent {
  return 'series_id' in event && typeof event.series_id === 'string';
}
