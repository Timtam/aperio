// Splitting a series at one of its occurrences — "this and all following".
//
// The move itself is two writes that must both happen or neither: the original
// series is truncated to end just before the chosen occurrence, and a NEW
// series takes over from there carrying the change. Between those two writes
// the appointment has a hole in it, and every detail of the arithmetic decides
// whether the two halves line up afterwards:
//
//   - RFC-5545 COUNT counts every slot the RULE generates, INCLUDING the ones
//     an EXDATE suppresses. Counting the visible occurrences before the cutoff
//     therefore undercounts, and the tail is created one occurrence too long —
//     a phantom appointment past the end of the series.
//   - EXDATEs at or after the cutoff belong to the TAIL. Left behind on the
//     truncated head they do nothing, and the tail resurrects an occurrence the
//     user had explicitly deleted (or doubles one they had moved, its
//     suppressing EXDATE gone).
//   - The tail is a CONTINUATION, so it keeps the master's zone verbatim,
//     floating zones included. Stamping only the tail makes the two halves
//     expand an hour apart across a DST boundary.
//   - Attendees have to hear about the truncation as well as the new tail. On
//     notify-flag providers a silent truncate leaves them holding the old
//     occurrences AND an invitation to the new ones.
//   - And if creating the tail fails, the head must be put back — with the same
//     notify flag, or their calendars stay diverged from the organiser's.
//
// That reasoning lived inline in both editors, and carrying an edit to a
// group's other copies would have made it four. It lives here now: the
// arithmetic in `planSeriesSplit`, the order and the recovery in
// `writeSeriesSplit`. What each caller still owns is the SHAPE of the row it
// creates — a copy keeps its own colour, reminders and calendar, and only the
// caller knows those.

import type { RecurringEventLike } from './recurrence';
import { expandEvent, splitRRuleForEdit } from './recurrence';

/** The least a master needs for its series to be split. */
export interface SplittableEvent extends RecurringEventLike {
  all_day: boolean;
}

/** The recurrence the tail series is created with. */
export interface TailRecurrence {
  rrule: string;
  exceptions: string[];
  tzid: string | null;
}

/** What splitting this series at this occurrence would do. */
export interface SeriesSplitPlan {
  /** The rule the ORIGINAL series keeps: everything strictly before the cutoff. */
  headRule: string;
  /** The recurrence the NEW tail series is created with. */
  tail: TailRecurrence;
  /**
   * How many occurrences the RULE generates before the cutoff.
   *
   * Exposed because it is the number the COUNT arithmetic turns on, and a test
   * that cannot see it can only check the split from the outside.
   */
  occurrencesBefore: number;
}

/**
 * The arithmetic of a split, decided before anything is written.
 *
 * `null` when there is nothing to split: the master carries no rule at all, or
 * the cutoff is not a moment. A caller that gets `null` for an event it
 * believed was a series must NOT fall through to editing the whole series —
 * that moves every occurrence, which is the outcome the scope question exists
 * to prevent.
 */
export function planSeriesSplit<E extends SplittableEvent>(
  master: E,
  cutoffIso: string,
): SeriesSplitPlan | null {
  const recurrence = master.recurrence;
  if (!recurrence?.rrule) return null;
  const cutoff = new Date(cutoffIso);
  if (!Number.isFinite(cutoff.getTime())) return null;

  // Occurrences the RULE generates strictly before the cutoff. Two subtleties:
  // (1) `expandEvent`'s range includes the end instant and the cutoff IS an
  // occurrence, so the range ends a millisecond earlier to exclude it;
  // (2) COUNT counts exdated slots too, so the expansion runs with the
  // exceptions cleared — otherwise an EXDATE before the cutoff undercounts and
  // the tail keeps a COUNT one too large.
  const occurrencesBefore = expandEvent(
    { ...master, recurrence: { ...recurrence, exceptions: [] } },
    { start: new Date(master.start), end: new Date(cutoff.getTime() - 1) },
  ).length;

  const { oldRule, newRule } = splitRRuleForEdit(
    recurrence.rrule,
    cutoff,
    occurrencesBefore,
    { allDay: master.all_day },
  );

  return {
    headRule: oldRule,
    tail: {
      rrule: newRule,
      // The EXDATEs at or after the cutoff move to the tail with the
      // occurrences they suppress.
      exceptions: (recurrence.exceptions ?? []).filter(
        (x) => new Date(x).getTime() >= cutoff.getTime(),
      ),
      tzid: recurrence.tzid ?? null,
    },
    occurrencesBefore,
  };
}

/**
 * The first occurrence of this series that is not yet OVER at `fromIso`.
 *
 * The anchor of a split is the occurrence the user picked, and on that copy it
 * IS an occurrence. On another copy of the same appointment it need not be:
 * copies are separate series and may be patterned differently, so the honest
 * anchor for "and all following" over there is that copy's own next occurrence.
 *
 * "Not yet over" rather than "starting at or after": copies of one appointment
 * often start a little apart — the work copy at 09:00 and the private one at
 * 08:45 for the walk over — and by START alone the copy that is already
 * running at the cutoff counted as past. It was then cut a whole period late:
 * today's appointment stayed as it was and next week's carried the change.
 *
 * `null` when the series has none left — the copy ends before the cutoff, and
 * there is nothing to carry to it. Reported to the user rather than treated as
 * success, because a copy that silently kept its old shape is exactly the
 * contradiction a group exists to prevent.
 */
export function firstOccurrenceFrom<E extends SplittableEvent>(
  master: E,
  fromIso: string,
): string | null {
  const from = new Date(fromIso);
  if (!Number.isFinite(from.getTime())) return null;
  const duration = Math.max(
    0,
    new Date(master.end).getTime() - new Date(master.start).getTime(),
  );
  if (!master.recurrence?.rrule) {
    // A single event is its own only occurrence — and only if it is not over.
    return new Date(master.end).getTime() > from.getTime() ? master.start : null;
  }
  // Widening rather than one fixed window. A weekly series answers in the first
  // step; a three-yearly one would have fallen outside any horizon short enough
  // to keep the common case cheap, and "no occurrence" is not a harmless answer
  // here — it tells the carry this copy has nothing left, and the copy is
  // reported as one it could not do. The last step reaches forty years out,
  // past any series a calendar sensibly holds.
  //
  // The range starts one duration EARLIER than the cutoff because `expandEvent`
  // selects by start: an occurrence already running at the cutoff begins before
  // it, and it is the one being split at, not the next one.
  const DAY_MS = 24 * 60 * 60 * 1000;
  const searchFrom = new Date(from.getTime() - duration);
  for (const days of [400, 1_200, 4_000, 15_000]) {
    const horizon = new Date(from.getTime() + days * DAY_MS);
    const found = expandEvent(master, { start: searchFrom, end: horizon }).find(
      (occ) => new Date(occ.start).getTime() + duration > from.getTime(),
    );
    if (found) return found.start;
  }
  return null;
}

/**
 * The three writes a split needs, in the caller's own terms.
 *
 * Deliberately narrow: the shaping of the rows is the caller's business (a copy
 * keeps its colour, its reminders and its calendar, and only the caller knows
 * what the user just typed), while the ORDER, the notify flag and the recovery
 * are the same everywhere and belong to `writeSeriesSplit`.
 */
export interface SeriesSplitIo<Created> {
  /**
   * Write the master back ending just before the cutoff.
   *
   * Must pass the notify flag on, and must ask the adapter to drop provider-side
   * overrides in the dropped tail — a cross-client single-occurrence change
   * synced in as its own RECURRENCE-ID row otherwise survives the truncation and
   * ghosts against the new series.
   */
  truncate(headRule: string): Promise<unknown>;
  /** Create the tail series with this recurrence, keeping the zone verbatim. */
  createTail(recurrence: TailRecurrence): Promise<Created>;
  /**
   * Put the master back as it was.
   *
   * Called only when the tail could not be created. Must carry the SAME notify
   * flag as `truncate`: if attendees were told the series ends early, they have
   * to be told it is whole again.
   */
  restore(): Promise<unknown>;
}

/** Marks a thrown error whose series was left truncated. */
const RESTORE_FAILED = Symbol.for('aperio.seriesSplit.restoreFailed');

/**
 * Whether this failure left the series SHORTER than it was.
 *
 * The ordinary failure of a split changes nothing: the tail could not be
 * created, the head went back as it was, and the caller reports an error over
 * an untouched calendar. When the restore fails too, the series really does end
 * at the cutoff now — every appointment from there on is gone. That is a
 * different thing to tell the user, and reporting it as "not changed" would be
 * the opposite of true.
 */
export function seriesLeftTruncated(err: unknown): boolean {
  return (
    typeof err === 'object' &&
    err !== null &&
    (err as Record<symbol, unknown>)[RESTORE_FAILED] === true
  );
}

/**
 * Truncate, then create the tail — and put the master back if the tail fails.
 *
 * The failure path is the reason this is one function. A tail that could not be
 * created leaves a series that silently ENDS at the cutoff: every appointment
 * from there on is simply gone, and nothing on screen says so. Restoring the
 * head turns that into an ordinary error the caller can report, with the
 * calendar exactly as it was.
 *
 * The ORIGINAL failure is what gets thrown either way — it is what the caller is
 * waiting for, and a second message about a failed repair would bury it. But a
 * failed restore is marked on it, so a caller that wants to say "and this one is
 * now short" can (`seriesLeftTruncated`).
 */
export async function writeSeriesSplit<Created>(
  io: SeriesSplitIo<Created>,
  plan: SeriesSplitPlan,
): Promise<Created> {
  await io.truncate(plan.headRule);
  try {
    return await io.createTail(plan.tail);
  } catch (err) {
    try {
      await io.restore();
    } catch {
      if (typeof err === 'object' && err !== null) {
        (err as Record<symbol, unknown>)[RESTORE_FAILED] = true;
      }
    }
    throw err;
  }
}
