import { describe, expect, it } from 'vitest';

/**
 * Where the phone puts focus after an action failed and its rows read again
 * (`focusAfterFailedAction`): the day-start review's buttons and the task
 * list's Delete. It waits for the re-read's own fetch — rows served before it
 * are older, in whatever array — and then watches for a moment: the refresh
 * that read starts may still take a row away whose write landed after all.
 * While a row the pressed button belongs to shows, focus stays on it — a row
 * button while its own row shows, a bulk button while a row of its batch
 * does — so the failure sentence is heard to its end, as on the desktop; once
 * none shows it moves on, and once the watch is over it stops. The phone has
 * no test runner of its own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_REVIEW_FOCUS = '../../mobile/src/state/reviewFocus';
type FailedAction = {
  acted: readonly string[];
  fallback: string | null;
  version: number;
  until: number;
};
type FocusStep = { kind: 'wait' } | { kind: 'stay' } | { kind: 'move'; id: string };
type FocusAfterFailedAction = (
  gone: FailedAction,
  orderedIds: readonly string[],
  fetched: number,
  now: number,
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
/** When the watch ends, a moment within it, and one after it. */
const UNTIL = 15_000;
const SOON = 1_000;
const LATER = 20_000;

const failed = (acted: string[], fallback: string | null): FailedAction => ({
  acted,
  fallback,
  version: AT,
  until: UNTIL,
});

describe('the phone focus after a failed action', () => {
  it('waits for the re-read, whatever the rows show meanwhile', async () => {
    // "All done" on A and B: A landed and left at once, B failed and still
    // shows from before — also in the cached rows served again, a new array
    // with the old rows; only the fetch at the failure's version counts.
    expect(await focusAfterFailedAction(failed(['a', 'b'], 'c'), [], BEFORE, SOON)).toEqual({
      kind: 'wait',
    });
  });

  it('keeps focus on a row button while its row shows, and watches', async () => {
    expect(await focusAfterFailedAction(failed(['a'], 'b'), ['a', 'b'], AFTER, SOON)).toEqual({
      kind: 'wait',
    });
  });

  it('moves on to the queued row when a read takes the row away', async () => {
    // The re-read served the cached rows; the refresh after it, a moment
    // later, shows the delete landed.
    expect(await focusAfterFailedAction(failed(['a'], 'b'), ['b'], AFTER + 1, SOON)).toEqual({
      kind: 'move',
      id: 'b',
    });
  });

  it("moves on from a row button whose row left, though its subtask now shows", async () => {
    // Today on R, whose subtask S was written with it and failed: R left, S
    // shows as a row of its own. Only R's showing keeps R's button.
    expect(await focusAfterFailedAction(failed(['r'], 'n'), ['s', 'n'], AFTER, SOON)).toEqual({
      kind: 'move',
      id: 'n',
    });
  });

  it('keeps focus on a bulk button while a row of its batch shows', async () => {
    expect(await focusAfterFailedAction(failed(['a', 'b'], 'c'), ['b', 'c'], AFTER, SOON)).toEqual({
      kind: 'wait',
    });
  });

  it('moves on once a read took every row of the batch away', async () => {
    // B's answer was lost, but it was completed: the section and its button
    // are gone.
    expect(await focusAfterFailedAction(failed(['a', 'b'], 'c'), ['c'], AFTER, SOON)).toEqual({
      kind: 'move',
      id: 'c',
    });
  });

  it('stops watching once the watch is over, wherever the row went', async () => {
    // Another device removes the row a minute later: focus is not pulled.
    expect(await focusAfterFailedAction(failed(['a'], 'b'), ['b'], AFTER + 3, LATER)).toEqual({
      kind: 'stay',
    });
  });

  it('stays when no row is left to move to', async () => {
    expect(await focusAfterFailedAction(failed(['a'], null), [], AFTER, SOON)).toEqual({
      kind: 'stay',
    });
  });
});
