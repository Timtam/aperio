/** A day-start review action that failed, as its focus waits on the re-read:
 *  the rows whose showing means the pressed button still shows (the pressed
 *  row for a row button, the batch for a bulk one), the row to take focus
 *  otherwise, and the data version the failure's re-read reads at. */
export type FailedAction = {
  acted: readonly string[];
  fallback: string | null;
  version: number;
};

/** What focus does once rows render after a failed action. */
export type FocusStep = { kind: 'wait' } | { kind: 'stay' } | { kind: 'move'; id: string };

/**
 * Where focus goes after an action of the day-start review failed and its
 * rows read again (decision 273). Until a fetch at the failure's version or
 * later has landed (`fetched`) it waits — rows served from the cache before
 * that are older, in whatever array — since what the provider holds decides
 * whether the pressed button is still there. Then it stays where the person
 * left it, on the pressed button, while a row in `acted` shows — the pressed
 * row, or for a bulk button any row of its batch — and otherwise moves on to
 * the row queued for after them.
 */
export function focusAfterFailedAction(
  gone: FailedAction,
  orderedIds: readonly string[],
  fetched: number,
): FocusStep {
  if (fetched < gone.version) return { kind: 'wait' };
  if (gone.acted.some((id) => orderedIds.includes(id))) return { kind: 'stay' };
  return gone.fallback == null ? { kind: 'stay' } : { kind: 'move', id: gone.fallback };
}
