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
 * The first occurrence this series has at or after `fromIso`.
 *
 * The anchor of a split is the occurrence the user picked, and on that copy it
 * IS an occurrence. On another copy of the same appointment it need not be:
 * copies are separate series and may be patterned differently, so the honest
 * anchor for "and all following" over there is that copy's own next occurrence.
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
  if (!master.recurrence?.rrule) {
    // A single event is its own only occurrence — and only if it is not behind
    // us already.
    return new Date(master.start).getTime() >= from.getTime()
      ? master.start
      : null;
  }
  // Widening rather than one fixed window. A weekly series answers in the first
  // step; a three-yearly one would have fallen outside any horizon short enough
  // to keep the common case cheap, and "no occurrence" is not a harmless answer
  // here — it tells the carry this copy has nothing left, and the copy is
  // reported as one it could not do. The last step reaches forty years out,
  // past any series a calendar sensibly holds.
  const DAY_MS = 24 * 60 * 60 * 1000;
  for (const days of [400, 1_200, 4_000, 15_000]) {
    const horizon = new Date(from.getTime() + days * DAY_MS);
    // `expandEvent` selects by start, so the first row it returns for a range
    // beginning at the cutoff is the occurrence at or after it.
    const first = expandEvent(master, { start: from, end: horizon })[0];
    if (first && new Date(first.start).getTime() >= from.getTime()) {
      return first.start;
    }
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

/**
 * Truncate, then create the tail — and put the master back if the tail fails.
 *
 * The failure path is the reason this is one function. A tail that could not be
 * created leaves a series that silently ENDS at the cutoff: every appointment
 * from there on is simply gone, and nothing on screen says so. Restoring the
 * head turns that into an ordinary error the caller can report, with the
 * calendar exactly as it was.
 *
 * A failing restore is deliberately swallowed: the original failure is the one
 * worth reporting, and it is the one the caller is waiting for.
 */
export async function writeSeriesSplit<Created>(
  io: SeriesSplitIo<Created>,
  plan: SeriesSplitPlan,
): Promise<Created> {
  await io.truncate(plan.headRule);
  try {
    return await io.createTail(plan.tail);
  } catch (err) {
    await io.restore().catch(() => undefined);
    throw err;
  }
}
