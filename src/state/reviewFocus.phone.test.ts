import { describe, expect, it } from 'vitest';

/**
 * Where the phone's day-start review puts focus after an action failed and
 * its rows read again (`focusAfterFailedAction`). It waits for the re-read,
 * since what the provider holds decides whether the pressed button is still
 * there. Then the pressed button keeps focus while it shows — a row button
 * while its own row shows, a bulk button while a row of its batch does — so
 * the failure sentence is heard to its end, as on the desktop. The phone has
 * no test runner of its own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_REVIEW_FOCUS = '../../mobile/src/state/reviewFocus';
type FailedAction = { acted: readonly string[]; fallback: string | null; version: number };
type FocusStep = { kind: 'wait' } | { kind: 'stay' } | { kind: 'move'; id: string };
type FocusAfterFailedAction = (
  gone: FailedAction,
  orderedIds: readonly string[],
  fetched: number,
) => FocusStep;
async function focusAfterFailedAction(
  ...args: Parameters<FocusAfterFailedAction>
): Promise<FocusStep> {
  const phoneModule = (await import(/* @vite-ignore */ PHONE_REVIEW_FOCUS)) as {
    focusAfterFailedAction: FocusAfterFailedAction;
  };
  return phoneModule.focusAfterFailedAction(...args);
}

/** The data version the failure's re-read reads at, the version of the last
 *  fetch before it (the rows on screen, or a cached copy of them served
 *  again), and the fetch the re-read brought. */
const AT = 7;
const BEFORE = AT - 1;
const AFTER = AT;

describe("the phone review's focus after a failed action", () => {
  it('waits for the re-read, whatever the rows show meanwhile', async () => {
    // "All done" on A and B: A landed and left at once, B failed and still
    // shows from before — also in the cached rows served again, a new array
    // with the old rows; only the fetch at the failure's version decides.
    const gone = { acted: ['a', 'b'], fallback: 'c', version: AT };
    expect(await focusAfterFailedAction(gone, ['b', 'c'], BEFORE)).toEqual({ kind: 'wait' });
  });

  it('decides on a fetch later than the one it waited for', async () => {
    const gone = { acted: ['a'], fallback: 'b', version: AT };
    expect(await focusAfterFailedAction(gone, ['b'], AT + 2)).toEqual({ kind: 'move', id: 'b' });
  });

  it('stays on a row button whose row still shows', async () => {
    const gone = { acted: ['a'], fallback: 'b', version: AT };
    expect(await focusAfterFailedAction(gone, ['a', 'b'], AFTER)).toEqual({ kind: 'stay' });
  });

  it('moves on to the queued row when the re-read took the row away', async () => {
    const gone = { acted: ['a'], fallback: 'b', version: AT };
    expect(await focusAfterFailedAction(gone, ['b'], AFTER)).toEqual({ kind: 'move', id: 'b' });
  });

  it("moves on from a row button whose row left, though its subtask now shows", async () => {
    // Today on R, whose subtask S was written with it and failed: R left, S
    // shows as a row of its own. Only R's showing keeps R's button.
    const gone = { acted: ['r'], fallback: 'n', version: AT };
    expect(await focusAfterFailedAction(gone, ['s', 'n'], AFTER)).toEqual({ kind: 'move', id: 'n' });
  });

  it('stays on a bulk button while a row of its batch still shows', async () => {
    const gone = { acted: ['a', 'b'], fallback: 'c', version: AT };
    expect(await focusAfterFailedAction(gone, ['b', 'c'], AFTER)).toEqual({ kind: 'stay' });
  });

  it('moves on once the re-read took every row of the batch away', async () => {
    // B's answer was lost, but it was completed: the section and its button
    // are gone.
    const gone = { acted: ['a', 'b'], fallback: 'c', version: AT };
    expect(await focusAfterFailedAction(gone, ['c'], AFTER)).toEqual({ kind: 'move', id: 'c' });
  });

  it('stays when no row is left to move to', async () => {
    const gone = { acted: ['a'], fallback: null, version: AT };
    expect(await focusAfterFailedAction(gone, [], AFTER)).toEqual({ kind: 'stay' });
  });
});
