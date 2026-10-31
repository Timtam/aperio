import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The phone's live-reload coalescer (`mobile/src/state/cacheReloadBus.ts`).
 * During an account refresh every flush used to reload every mounted screen,
 * the app badge, the widget, and start the reminder scan, and all of it
 * queued in front of the reads of the screen the user was paging. Now only
 * the screen in view reloads at a flush, a screen out of view once on its
 * return, and what nobody looks at during a pass waits for its end. The phone
 * has no test runner of its own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_BUS = '../../mobile/src/state/cacheReloadBus';

interface ScreenFocus {
  isFocused(): boolean;
  addListener(event: 'focus', cb: () => void): () => void;
}
interface Bus {
  subscribeScreenReload(
    category: string,
    reload: () => void,
    screen: ScreenFocus | undefined,
  ): () => void;
  noteCacheUpdated(scope: string): void;
  notePassRefreshing(refreshing: boolean): void;
  notifyDataReload(): void;
  setRemindersRefreshHook(cb: () => void): void;
  whenPassSettled(work: () => void): void;
  cancelWhenPassSettled(work: () => void): void;
  runSettledWork(): void;
  COALESCE_MS: number;
  PASS_THROTTLE_MS: number;
  SETTLE_CAP_MS: number;
}

/** A fresh module per test: its state is module-level. */
async function freshBus(): Promise<Bus> {
  vi.resetModules();
  return (await import(/* @vite-ignore */ PHONE_BUS)) as Bus;
}

/** A screen whose focus the test moves, with its focus listeners. */
function screen(focused: boolean) {
  const listeners = new Set<() => void>();
  return {
    focused,
    isFocused() {
      return this.focused;
    },
    addListener(_event: 'focus', cb: () => void) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    focus() {
      this.focused = true;
      listeners.forEach((cb) => cb());
    },
    listeners,
  };
}

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe('a screen reloads while it is in view', () => {
  it('reloads the focused screen at each flush', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    bus.subscribeScreenReload('calendar', reload, screen(true));
    bus.noteCacheUpdated('events');
    vi.advanceTimersByTime(bus.COALESCE_MS);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('a screen out of view reloads once, on its return', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    const tab = screen(false);
    bus.subscribeScreenReload('calendar', reload, tab);
    bus.noteCacheUpdated('events');
    vi.advanceTimersByTime(bus.COALESCE_MS);
    bus.noteCacheUpdated('calendars');
    vi.advanceTimersByTime(bus.COALESCE_MS);
    expect(reload).not.toHaveBeenCalled();
    tab.focus();
    expect(reload).toHaveBeenCalledTimes(1);
    // Its next return finds nothing missed.
    tab.focused = false;
    tab.focus();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('a return after no flush does not reload (the screen does that itself)', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    const tab = screen(false);
    bus.subscribeScreenReload('calendar', reload, tab);
    tab.focus();
    expect(reload).not.toHaveBeenCalled();
  });

  it('only the category subscribed to', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    const tab = screen(false);
    bus.subscribeScreenReload('contacts', reload, tab);
    bus.noteCacheUpdated('events');
    vi.advanceTimersByTime(bus.COALESCE_MS);
    tab.focus();
    expect(reload).not.toHaveBeenCalled();
  });

  it('a store outside any screen reloads at every flush', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    bus.subscribeScreenReload('tasks', reload, undefined);
    bus.noteCacheUpdated('tasks');
    vi.advanceTimersByTime(bus.COALESCE_MS);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('unsubscribing drops the focus listener too', async () => {
    const bus = await freshBus();
    const tab = screen(false);
    const off = bus.subscribeScreenReload('calendar', vi.fn(), tab);
    expect(tab.listeners.size).toBe(1);
    off();
    expect(tab.listeners.size).toBe(0);
  });
});

describe('what waits for the end of a pass', () => {
  it('runs at once when no pass runs', async () => {
    const bus = await freshBus();
    const work = vi.fn();
    bus.whenPassSettled(work);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it('runs once at the end of a pass, however often it was asked for', async () => {
    const bus = await freshBus();
    const work = vi.fn();
    bus.notePassRefreshing(true);
    bus.whenPassSettled(work);
    bus.whenPassSettled(work);
    bus.whenPassSettled(work);
    expect(work).not.toHaveBeenCalled();
    bus.notePassRefreshing(false);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it('work the pass-end flush defers joins that one run', async () => {
    const bus = await freshBus();
    const work = vi.fn();
    // A badge: subscribed outside any screen, deferring its compute.
    bus.subscribeScreenReload('tasks', () => bus.whenPassSettled(work), undefined);
    bus.notePassRefreshing(true);
    bus.noteCacheUpdated('tasks');
    vi.advanceTimersByTime(bus.PASS_THROTTLE_MS);
    bus.noteCacheUpdated('tasks');
    expect(work).not.toHaveBeenCalled();
    // The end flushes the pending 'tasks' first, then runs what waited.
    bus.notePassRefreshing(false);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it('runs at the cap when the pass never reports its end', async () => {
    const bus = await freshBus();
    const work = vi.fn();
    bus.notePassRefreshing(true);
    bus.whenPassSettled(work);
    vi.advanceTimersByTime(bus.SETTLE_CAP_MS - 1);
    expect(work).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(work).toHaveBeenCalledTimes(1);
    // The pass still runs: the next ask waits again.
    bus.whenPassSettled(work);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it('runs early when asked (the app leaves the foreground)', async () => {
    const bus = await freshBus();
    const work = vi.fn();
    bus.notePassRefreshing(true);
    bus.whenPassSettled(work);
    bus.runSettledWork();
    expect(work).toHaveBeenCalledTimes(1);
    bus.notePassRefreshing(false);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it('drops work its owner took back', async () => {
    const bus = await freshBus();
    const work = vi.fn();
    bus.notePassRefreshing(true);
    bus.whenPassSettled(work);
    bus.cancelWhenPassSettled(work);
    bus.notePassRefreshing(false);
    expect(work).not.toHaveBeenCalled();
  });
});

describe('the flush cadence', () => {
  it('throttles a pass, and its end flushes at once', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    bus.subscribeScreenReload('calendar', reload, screen(true));
    bus.notePassRefreshing(true);
    bus.noteCacheUpdated('events');
    vi.advanceTimersByTime(bus.COALESCE_MS);
    expect(reload).not.toHaveBeenCalled();
    vi.advanceTimersByTime(bus.PASS_THROTTLE_MS - bus.COALESCE_MS);
    expect(reload).toHaveBeenCalledTimes(1);
    bus.noteCacheUpdated('events');
    bus.notePassRefreshing(false);
    expect(reload).toHaveBeenCalledTimes(2);
  });

  it('a peer round reloads at once and asks for a reminder re-plan', async () => {
    const bus = await freshBus();
    const reload = vi.fn();
    const hook = vi.fn();
    bus.setRemindersRefreshHook(hook);
    bus.subscribeScreenReload('sections', reload, screen(true));
    bus.notifyDataReload();
    expect(reload).toHaveBeenCalledTimes(1);
    expect(hook).toHaveBeenCalledTimes(1);
  });
});
