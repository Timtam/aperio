/** How long after a failed action focus keeps watching its rows: the re-read
 *  first serves what the host cached, and the background refresh it starts
 *  brings what the provider holds a moment later (decision 271). */
export const FAILED_FOCUS_WATCH_MS = 15_000;

/** An action that failed, as its focus watches the rows read again: the rows
 *  whose showing means the pressed button still shows (the pressed row for a
 *  row button, the batch for a bulk one), the row to take focus otherwise
 *  (`null`: none left), the data version the failure's re-read reads at, and
 *  until when it watches. */
export type FailedAction = {
  acted: readonly string[];
  fallback: string | null;
  version: number;
  until: number;
};

/** What focus does once rows render after a failed action: keep watching,
 *  stop watching where it is, or move on. */
export type FocusStep = { kind: 'wait' } | { kind: 'stay' } | { kind: 'move'; id: string };

/**
 * Where focus goes after an action failed and its rows read again (decision
 * 273). Until a fetch at the failure's version or later has landed
 * (`fetched`) it waits — rows served before it are older, in whatever array.
 * Then, while the watch lasts, focus stays where the person left it, on the
 * pressed button, as long as a row in `acted` shows — the pressed row, or for
 * a bulk button any row of its batch — and moves on to the queued row once
 * none does: the refresh after the re-read may still take a row away whose
 * write landed after all. Once the watch is over it stops, so a row another
 * device removes later never pulls focus away.
 */
export function focusAfterFailedAction(
  gone: FailedAction,
  orderedIds: readonly string[],
  fetched: number,
  now: number,
): FocusStep {
  if (fetched < gone.version) return { kind: 'wait' };
  if (now >= gone.until) return { kind: 'stay' };
  if (gone.acted.some((id) => orderedIds.includes(id))) return { kind: 'wait' };
  return gone.fallback == null ? { kind: 'stay' } : { kind: 'move', id: gone.fallback };
}
