/** A day-start review action that failed, as its focus waits on the re-read:
 *  the rows whose showing means the pressed button still shows (the pressed
 *  row for a row button, the batch for a bulk one), the row to take focus
 *  otherwise, and the rows on screen when it failed. */
export type FailedAction = {
  acted: readonly string[];
  fallback: string | null;
  rows: unknown;
};

/** What focus does once rows render after a failed action. */
export type FocusStep = { kind: 'wait' } | { kind: 'stay' } | { kind: 'move'; id: string };

/**
 * Where focus goes after an action of the day-start review failed and its
 * rows read again (decision 273). Until the re-read has come — the rows on
 * screen are still the ones from before the failure — it waits: what the
 * provider holds decides whether the pressed button is still there. Then it
 * stays where the person left it, on the pressed button, while a row in
 * `acted` shows — the pressed row, or for a bulk button any row of its batch
 * — and otherwise moves on to the row queued for after them.
 */
export function focusAfterFailedAction(
  gone: FailedAction,
  orderedIds: readonly string[],
  rows: unknown,
): FocusStep {
  if (rows === gone.rows) return { kind: 'wait' };
  if (gone.acted.some((id) => orderedIds.includes(id))) return { kind: 'stay' };
  return gone.fallback == null ? { kind: 'stay' } : { kind: 'move', id: gone.fallback };
}
