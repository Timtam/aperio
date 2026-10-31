import { describe, expect, it } from 'vitest';

/**
 * The phone's `withFollowUp` (`mobile/src/state/followUpRun.ts`), behind the
 * reminder scan: a call during a run used to be dropped, so a scan that read
 * before a check-off kept the task's reminder until the next occasion (TODO
 * B13, 218). The phone has no test runner of its own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_FOLLOW_UP = '../../mobile/src/state/followUpRun';
type WithFollowUp = (run: () => Promise<void>) => () => Promise<void>;
async function withFollowUp(run: () => Promise<void>): Promise<() => Promise<void>> {
  const phoneModule = (await import(/* @vite-ignore */ PHONE_FOLLOW_UP)) as {
    withFollowUp: WithFollowUp;
  };
  return phoneModule.withFollowUp(run);
}

/** A run the test lets finish, one at a time. */
function gatedRun() {
  const gates: (() => void)[] = [];
  let started = 0;
  const run = () =>
    new Promise<void>((resolve) => {
      started += 1;
      gates.push(resolve);
    });
  return {
    run,
    started: () => started,
    finishNext: async () => {
      gates.shift()?.();
      // Let the loop pick up its next round.
      await new Promise((r) => setTimeout(r, 0));
    },
  };
}

describe('withFollowUp', () => {
  it('runs once when nothing else asks', async () => {
    const g = gatedRun();
    const go = await withFollowUp(g.run);
    const done = go();
    expect(g.started()).toBe(1);
    await g.finishNext();
    await done;
    expect(g.started()).toBe(1);
  });

  it('a call during a run brings exactly one more, however many came', async () => {
    const g = gatedRun();
    const go = await withFollowUp(g.run);
    const first = go();
    void go();
    void go();
    // No second run beside the first.
    expect(g.started()).toBe(1);
    await g.finishNext();
    expect(g.started()).toBe(2);
    await g.finishNext();
    await first;
    expect(g.started()).toBe(2);
  });

  it("a later caller's promise waits for the follow-up", async () => {
    const g = gatedRun();
    const go = await withFollowUp(g.run);
    void go();
    let laterDone = false;
    const later = go().then(() => {
      laterDone = true;
    });
    await g.finishNext();
    expect(laterDone).toBe(false);
    await g.finishNext();
    await later;
    expect(laterDone).toBe(true);
  });

  it('starts afresh once a run is over', async () => {
    const g = gatedRun();
    const go = await withFollowUp(g.run);
    const first = go();
    await g.finishNext();
    await first;
    void go();
    expect(g.started()).toBe(2);
    await g.finishNext();
  });
});
