import { describe, expect, it } from 'vitest';

/**
 * Where the phone's day-start review puts focus after an action failed and
 * its rows read again (`focusAfterFailedAction`). The pressed button keeps
 * focus while any row it worked on shows — for a bulk button, a row of its
 * batch the provider turned away — so the failure sentence is heard to its
 * end, as on the desktop. The phone has no test runner of its own, so its
 * module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_REVIEW_FOCUS = '../../mobile/src/state/reviewFocus';
type FocusAfterFailedAction = (
  gone: { acted: readonly string[]; fallback: string | null },
  orderedIds: readonly string[],
) => string | null;
async function focusAfterFailedAction(
  ...args: Parameters<FocusAfterFailedAction>
): Promise<string | null> {
  const phoneModule = (await import(/* @vite-ignore */ PHONE_REVIEW_FOCUS)) as {
    focusAfterFailedAction: FocusAfterFailedAction;
  };
  return phoneModule.focusAfterFailedAction(...args);
}

describe("the phone review's focus after a failed action", () => {
  it('stays on a row button whose row still shows', async () => {
    expect(await focusAfterFailedAction({ acted: ['a'], fallback: 'b' }, ['a', 'b'])).toBeNull();
  });

  it('moves on to the queued row when the re-read took the row away', async () => {
    expect(await focusAfterFailedAction({ acted: ['a'], fallback: 'b' }, ['b'])).toBe('b');
  });

  it('stays on a bulk button while a row of its batch still shows', async () => {
    // "All done" on A and B: A landed and left, B was turned away and stays.
    expect(
      await focusAfterFailedAction({ acted: ['a', 'b'], fallback: 'c' }, ['b', 'c']),
    ).toBeNull();
  });

  it('moves on once every row of the batch left', async () => {
    expect(await focusAfterFailedAction({ acted: ['a', 'b'], fallback: 'c' }, ['c'])).toBe('c');
  });
});
