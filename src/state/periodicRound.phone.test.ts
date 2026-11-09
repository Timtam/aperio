import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The phone's foreground round on the one refresh interval
 * (`mobile/src/api/periodicRound.ts`): every round refreshes the external
 * data, with or without a sync target, and syncs only with one. The phone has
 * no test runner of its own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_ROUND = '../../mobile/src/api/periodicRound';

interface Deps {
  refresh: () => Promise<void>;
  syncIfConfigured: () => Promise<void>;
  intervalMinutes: () => Promise<number>;
  isActive: () => boolean;
  setTimer: (run: () => void, ms: number) => unknown;
  clearTimer: (timer: unknown) => void;
}
interface Loop {
  start: () => void;
  stop: () => void;
  rearm: () => void;
}
async function phone() {
  return (await import(/* @vite-ignore */ PHONE_ROUND)) as {
    runRound: (deps: Pick<Deps, 'refresh' | 'syncIfConfigured'>) => Promise<void>;
    createRoundLoop: (deps: Deps) => Loop;
  };
}

function deps(minutes: { value: number }) {
  return {
    refresh: vi.fn(() => Promise.resolve()),
    syncIfConfigured: vi.fn(() => Promise.resolve()),
    intervalMinutes: vi.fn(() => Promise.resolve(minutes.value)),
    isActive: vi.fn(() => true),
    setTimer: (run: () => void, ms: number) => setTimeout(run, ms),
    clearTimer: (timer: unknown) => clearTimeout(timer as ReturnType<typeof setTimeout>),
  };
}

const MINUTE = 60_000;

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe('a round', () => {
  it('refreshes the external data even when the sync round fails or has no target', async () => {
    const { runRound } = await phone();
    const refresh = vi.fn(() => Promise.resolve());
    const syncIfConfigured = vi.fn(() => Promise.reject(new Error('no target')));
    await expect(runRound({ refresh, syncIfConfigured })).resolves.toBeUndefined();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(syncIfConfigured).toHaveBeenCalledTimes(1);
  });
});

describe('the round loop', () => {
  it('runs a round once per interval, read before every wait', async () => {
    const { createRoundLoop } = await phone();
    const minutes = { value: 15 };
    const d = deps(minutes);
    createRoundLoop(d).start();
    await vi.advanceTimersByTimeAsync(15 * MINUTE - 1);
    expect(d.refresh).not.toHaveBeenCalled();
    // Changed on another device during the wait: this wait stays, the next
    // one follows it.
    minutes.value = 5;
    await vi.advanceTimersByTimeAsync(1);
    expect(d.refresh).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5 * MINUTE);
    expect(d.refresh).toHaveBeenCalledTimes(2);
  });

  it('measures the wait again at once when the interval changes here', async () => {
    const { createRoundLoop } = await phone();
    const minutes = { value: 240 };
    const d = deps(minutes);
    const loop = createRoundLoop(d);
    loop.start();
    await vi.advanceTimersByTimeAsync(10 * MINUTE);
    minutes.value = 1;
    loop.rearm();
    await vi.advanceTimersByTimeAsync(MINUTE);
    expect(d.refresh).toHaveBeenCalledTimes(1);
  });

  it('arms one timer when a rearm comes while the wait is still being read', async () => {
    const { createRoundLoop } = await phone();
    const minutes = { value: 1 };
    const d = deps(minutes);
    const armed = vi.fn(d.setTimer);
    const loop = createRoundLoop({ ...d, setTimer: armed });
    loop.start();
    loop.rearm();
    loop.rearm();
    // The three reads land; only the last arm may arm a timer.
    await vi.advanceTimersByTimeAsync(0);
    expect(armed).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(MINUTE);
    expect(d.refresh).toHaveBeenCalledTimes(1);
  });

  it('runs nothing once stopped, or while the app is not in front', async () => {
    const { createRoundLoop } = await phone();
    const minutes = { value: 1 };
    const stopped = deps(minutes);
    const loop = createRoundLoop(stopped);
    loop.start();
    loop.stop();
    const away = deps(minutes);
    away.isActive.mockReturnValue(false);
    createRoundLoop(away).start();
    await vi.advanceTimersByTimeAsync(5 * MINUTE);
    expect(stopped.refresh).not.toHaveBeenCalled();
    expect(away.refresh).not.toHaveBeenCalled();
  });
});
