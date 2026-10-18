// A series created in steps (decisions 202-213, 227, 265).
//
// Exchange creates a series without its deleted occurrences; the adapter then
// deletes them again one by one, proves it, and only then invites — which can
// take up to half a minute. The calendar says so on its row
// (`recurrence_capabilities.deletions_in_steps`, from the adapter's manifest),
// and every surface that creates such a series shows a "please wait" dialog
// while it runs: decided here, once.

import type { RecurrenceCapabilities } from './generated/RecurrenceCapabilities';

/** The calendar a series is created in, as far as this asks. */
export interface StepWriteTarget {
  recurrence_capabilities?: Pick<RecurrenceCapabilities, 'deletions_in_steps'> | null;
}

/** The recurrence a series is created with, as far as this asks. */
export interface StepWriteRecurrence {
  rrule?: string | null;
  exceptions?: readonly unknown[] | null;
}

/**
 * Whether CREATING this series in that calendar runs in steps that can take up
 * to half a minute: the calendar's adapter deletes a new series' deleted
 * occurrences one by one, and the series carries some. Asked only where a
 * series is created — a split's new part, a move, a copy, a duplicate, the
 * editor's calendar change —, never for a save in place (265).
 *
 * Deletions before today are not written again (263), and the adapter decides
 * which those are; a series whose deletions all lie in the past still shows
 * the dialog, briefly.
 */
export function createsInSteps(
  target: StepWriteTarget | null | undefined,
  recurrence: StepWriteRecurrence | null | undefined,
): boolean {
  return Boolean(
    target?.recurrence_capabilities?.deletions_in_steps &&
      recurrence?.rrule &&
      (recurrence.exceptions?.length ?? 0) > 0,
  );
}

/**
 * How a surface waits for a write: `write` runs either way; where `inSteps`
 * holds, the surface shows its "please wait" dialog while it runs and closes it
 * before the promise settles, so whatever the caller says or opens next comes
 * after the dialog is gone.
 */
export type StepWriteWait = <T>(inSteps: boolean, write: () => Promise<T>) => Promise<T>;

/** A surface without a dialog: the write just runs. */
export const writeWithoutWaiting: StepWriteWait = (_inSteps, write) => write();

/**
 * How a move, a copy or a duplicate waits while it creates a series in steps
 * (decisions 227, 265): the surface's wait, and the row of the calendar it
 * writes to. Absent, the write just runs.
 */
export interface StepWriteOptions {
  wait?: StepWriteWait;
  target?: StepWriteTarget | null;
}

/** Marks a failure a surface already said in its "please wait" dialog. */
const SHOWN_IN_NOTICE = Symbol.for('aperio.stepWrite.shownInNotice');

/**
 * The failure as thrown, marked as said on screen: the "please wait" dialog of
 * a surface without a dialog of its own (a drop in the sidebar, Ctrl+D) turns
 * into the notice that says it (decision 210), and the caller does not say it
 * a second time.
 */
export function markShownInNotice(err: unknown): unknown {
  const carrier = typeof err === 'object' && err !== null ? err : new Error(String(err));
  (carrier as Record<symbol, unknown>)[SHOWN_IN_NOTICE] = true;
  return carrier;
}

/** Whether a surface already said this failure in its notice. */
export function shownInNotice(err: unknown): boolean {
  return (
    typeof err === 'object' &&
    err !== null &&
    (err as Record<symbol, unknown>)[SHOWN_IN_NOTICE] === true
  );
}
