import { describe, expect, it } from 'vitest';

import type { StepWriteWait } from '@aperio/shared';

/**
 * The phone's "please wait" bridge (decisions 227, 265), run here because the
 * phone has no test runner: the host is shown while a write in steps runs, and
 * the wait settles only after the host says the dialog is gone — on iOS after
 * its dismissal, so what follows never presents over a dialog still leaving.
 *
 * Loaded through a path the type checker does not follow: the desktop's tsc
 * must not pull the phone's module graph in (CI has no mobile packages).
 */
const PHONE_MODULE = '../../mobile/src/state/stepWriteWait';

interface PhoneBridge {
  registerStepWriteWaitHost: (host: { open: () => void; close: () => Promise<void> }) => () => void;
  stepWriteWait: StepWriteWait;
  holdStepWriteWait: () => () => Promise<void>;
}

async function bridge(): Promise<PhoneBridge> {
  return (await import(/* @vite-ignore */ PHONE_MODULE)) as PhoneBridge;
}

/** A host that records what it was asked, and closes when told to. */
function recordingHost() {
  const log: string[] = [];
  let finishClose = () => {};
  const host = {
    open: () => {
      log.push('open');
    },
    close: () =>
      new Promise<void>((resolve) => {
        log.push('close asked');
        finishClose = () => {
          log.push('gone');
          resolve();
        };
      }),
  };
  return { host, log, gone: () => finishClose() };
}

describe("the phone's step-write wait", () => {
  it('just writes without a host, or for a write not in steps', async () => {
    const { stepWriteWait, registerStepWriteWaitHost } = await bridge();
    await expect(stepWriteWait(true, async () => 1)).resolves.toBe(1);
    const { host, log } = recordingHost();
    const unregister = registerStepWriteWaitHost(host);
    await expect(stepWriteWait(false, async () => 2)).resolves.toBe(2);
    expect(log).toEqual([]);
    unregister();
  });

  it('settles only once the host says the dialog is gone', async () => {
    const { stepWriteWait, registerStepWriteWaitHost } = await bridge();
    const { host, log, gone } = recordingHost();
    const unregister = registerStepWriteWaitHost(host);
    let settled = false;
    const waited = stepWriteWait(true, async () => 'written').then((v) => {
      settled = true;
      return v;
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(['open', 'close asked']);
    expect(settled).toBe(false);
    gone();
    await expect(waited).resolves.toBe('written');
    expect(log).toEqual(['open', 'close asked', 'gone']);
    unregister();
  });

  it('closes after a failed write too, and passes the failure on', async () => {
    const { stepWriteWait, registerStepWriteWaitHost } = await bridge();
    const { host, log, gone } = recordingHost();
    const unregister = registerStepWriteWaitHost(host);
    const waited = stepWriteWait(true, async () => {
      throw new Error('refused');
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    gone();
    await expect(waited).rejects.toThrow('refused');
    expect(log).toEqual(['open', 'close asked', 'gone']);
    unregister();
  });

  it('opens once across held writes and closes on the release', async () => {
    const { stepWriteWait, registerStepWriteWaitHost, holdStepWriteWait } = await bridge();
    const { host, log, gone } = recordingHost();
    const unregister = registerStepWriteWaitHost(host);
    const release = holdStepWriteWait();
    await stepWriteWait(true, async () => undefined);
    await stepWriteWait(false, async () => undefined);
    await stepWriteWait(true, async () => undefined);
    expect(log).toEqual(['open']);
    const released = release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    gone();
    await released;
    expect(log).toEqual(['open', 'close asked', 'gone']);
    unregister();
  });
});
