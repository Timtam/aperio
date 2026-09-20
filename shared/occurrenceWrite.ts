import {
  isProviderOverride,
  occurrenceIsoOf,
  overrideIdFor,
  seriesIdOf,
  type RecurringEventLike,
} from './recurrence';

/**
 * What a save or a drag of ONE occurrence does to the series it belongs to
 * (decision 79b).
 *
 * Until now every surface did the same thing: cut the occurrence out of its
 * series with an EXDATE and put a standalone event in its place. That loses
 * what makes it an occurrence — the series no longer knows it, a later edit of
 * the series leaves it behind, and on a provider that mails its guests the
 * carve-out is a cancellation plus a new invitation. Where the provider can
 * keep the changed occurrence INSIDE the series (`stores_occurrence_exceptions`:
 * CalDAV's `RECURRENCE-ID`, Exchange's modified occurrence, Google's instance),
 * that is what the surfaces write.
 *
 * One rule, because six call sites asked the question and each answered it
 * slightly differently: both editors, both drag paths, the group carry and the
 * delete paths.
 */
export type OccurrenceWrite =
  /** Write the occurrence itself, through `id`: it is already an exception, or
   *  the calendar can hold one. `occurrence` is the slot it stands in. */
  | { kind: 'in-place'; id: string; occurrence: string }
  /** Skip the slot in the series and put a standalone event there. */
  | { kind: 'carve-out'; seriesId: string; occurrence: string }
  /** Not this rule's business: not an occurrence, no readable slot, or a scope
   *  that means the series itself. */
  | { kind: 'series' };

/** The one capability this rule reads (`host_core::wire::CalendarRow`). */
export interface ExceptionCalendar {
  stores_occurrence_exceptions?: boolean;
}

/**
 * Which of the three a surface is about to do.
 *
 * `row` is what the surface holds — an expanded occurrence, a provider
 * override, or (on the phone) the series master its editor loaded. `occurrence`
 * is the slot when the surface carries it apart from the row, which the phone
 * always does: its editor opens the MASTER and remembers the slot beside it, so
 * without this the rule would answer "series" for every phone edit.
 *
 * `destination` says where the write goes. A move or a copy to ANOTHER calendar
 * can never be an exception: the series is not going with it, and an exception
 * only exists inside its series' own resource.
 */
export function occurrenceWrite<E extends RecurringEventLike>(input: {
  row: E;
  occurrence?: string | null;
  calendar: ExceptionCalendar | null | undefined;
  scope: 'occurrence' | 'this_and_future' | 'series';
  destination?: 'in-series' | 'another-calendar';
}): OccurrenceWrite {
  const { row, calendar, scope, destination = 'in-series' } = input;
  // "This and all following" truncates the series and starts another one; it
  // never touches a single occurrence, and it must not be swallowed here.
  if (scope !== 'occurrence') return { kind: 'series' };
  const occurrence = input.occurrence ?? occurrenceIsoOf(row);
  if (occurrence == null) return { kind: 'series' };
  // The series stays where it is, so the copy cannot be an exception of it.
  if (destination === 'another-calendar') {
    return { kind: 'carve-out', seriesId: seriesIdOf(row), occurrence };
  }
  // The row IS the exception already: write it through its own id, whatever a
  // listing says the calendar can do. A stale capability must not turn a write
  // that works today into a carve-out.
  if (isProviderOverride(row)) {
    return { kind: 'in-place', id: row.id, occurrence };
  }
  if (calendar?.stores_occurrence_exceptions === true) {
    return {
      kind: 'in-place',
      id: overrideIdFor(seriesIdOf(row), occurrence),
      occurrence,
    };
  }
  return { kind: 'carve-out', seriesId: seriesIdOf(row), occurrence };
}
