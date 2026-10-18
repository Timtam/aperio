import { useEffect } from 'react';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';

import type { StepWriteWait } from '@aperio/shared';

import { useStepWriteWait } from './useStepWriteWait';

/**
 * The "please wait" dialog's life (decisions 227, 265): open while a write in
 * steps runs, closed before the wait settles, and held across a carry's copies
 * so it opens and says itself once.
 */

let api: { wait: StepWriteWait; hold: () => () => Promise<void> } | null = null;

// What is tested is the browser's own order: the close commits and focus goes
// back before the wait settles. Inside act() React holds every update until
// the act ends, so that order cannot be seen there, and outside it React warns.
// So this file runs React as the browser does, without act's queue.
const reactAct = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
let wasActEnvironment: boolean | undefined;
beforeAll(() => {
  wasActEnvironment = reactAct.IS_REACT_ACT_ENVIRONMENT;
  reactAct.IS_REACT_ACT_ENVIRONMENT = false;
});
afterAll(() => {
  reactAct.IS_REACT_ACT_ENVIRONMENT = wasActEnvironment;
});

function Surface() {
  const { waitDialog, wait, hold } = useStepWriteWait();
  useEffect(() => {
    api = { wait, hold };
  }, [wait, hold]);
  return <div id="app-root">{waitDialog}</div>;
}

const dialog = () => screen.queryByRole('dialog', { name: /Vorgang läuft|please wait/i });

function gate() {
  let open = () => {};
  const done = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { done, open };
}

describe('useStepWriteWait', () => {
  it('shows nothing for a write that does not run in steps', async () => {
    render(<Surface />);
    const g = gate();
    let settled = false;
    void api!.wait(false, () => g.done).then(() => {
      settled = true;
    });
    expect(dialog()).toBeNull();
    await act(async () => g.open());
    await waitFor(() => expect(settled).toBe(true));
  });

  it('is open while the write runs and gone before the wait settles', async () => {
    render(<Surface />);
    const g = gate();
    let goneWhenSettled: boolean | null = null;
    const waited = api!.wait(true, () => g.done).then(() => {
      goneWhenSettled = dialog() === null;
    });
    await waitFor(() => expect(dialog()).not.toBeNull());
    await act(async () => g.open());
    await act(async () => waited);
    expect(goneWhenSettled).toBe(true);
  });

  it('closes after a failed write too, and passes the failure on', async () => {
    render(<Surface />);
    const waited = api!.wait(true, async () => {
      throw new Error('refused');
    });
    await expect(waited).rejects.toThrow('refused');
    await waitFor(() => expect(dialog()).toBeNull());
  });

  it('stays open across held writes and closes on the release', async () => {
    render(<Surface />);
    const release = api!.hold();
    const first = gate();
    const one = api!.wait(true, () => first.done);
    await waitFor(() => expect(dialog()).not.toBeNull());
    await act(async () => {
      first.open();
      await one;
    });
    // Between the copies it stays.
    expect(dialog()).not.toBeNull();
    await act(async () => {
      await api!.wait(false, async () => undefined);
      await api!.wait(true, async () => undefined);
    });
    expect(dialog()).not.toBeNull();
    let goneWhenReleased: boolean | null = null;
    const released = release().then(() => {
      goneWhenReleased = dialog() === null;
    });
    await act(async () => released);
    expect(goneWhenReleased).toBe(true);
  });
});
