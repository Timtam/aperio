import { act, render, screen } from '@testing-library/react';
import { useRef } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * A page turn (Back, Today, Forward, Ctrl+←/→, Ctrl+T) is said, unless the
 * view's own widget says it (TODO B15 step 3): the focus is in it, the period
 * moved, and it has something to read.
 */

const h = vi.hoisted(() => {
  type State = { pageTurn: number; anchor: Date };
  let state: State = { pageTurn: 0, anchor: new Date(2026, 10, 4) };
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    set: (patch: Partial<State>) => {
      state = { ...state, ...patch };
      listeners.forEach((l) => l());
    },
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    announce: vi.fn(),
  };
});

vi.mock('../state/viewStateContext', async () => {
  const { useSyncExternalStore } = await import('react');
  return { useViewState: () => useSyncExternalStore(h.subscribe, h.get) };
});
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));

beforeEach(() => {
  h.set({ pageTurn: 0, anchor: new Date(2026, 10, 4) });
  h.announce.mockReset();
});

async function renderView(widgetSpeaks = true) {
  const { usePageTurnAnnouncement } = await import('./usePageTurnAnnouncement');
  function View() {
    const widget = useRef<HTMLDivElement>(null);
    const { anchor } = h.get();
    usePageTurnAnnouncement(`Period of ${anchor.getDate()}`, widget, widgetSpeaks);
    return (
      <>
        <button type="button">Forward</button>
        <div ref={widget} role="grid" tabIndex={0} aria-label="Grid" />
      </>
    );
  }
  render(<View />);
}

/** A page turn: the anchor moves `days` and the count goes up. */
function turn(days: number) {
  const anchor = new Date(h.get().anchor);
  anchor.setDate(anchor.getDate() + days);
  act(() => h.set({ anchor, pageTurn: h.get().pageTurn + 1 }));
}

describe('a page turn', () => {
  it('is said with the focus outside the widget', async () => {
    await renderView();
    act(() => screen.getByRole('button').focus());
    turn(7);
    expect(h.announce).toHaveBeenCalledWith('Period of 11');
  });

  it('is left to the widget when the focus is in it and the period moved', async () => {
    await renderView();
    act(() => screen.getByRole('grid').focus());
    turn(7);
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('is said in the widget when nothing moved: Today on today', async () => {
    await renderView();
    act(() => screen.getByRole('grid').focus());
    turn(0);
    expect(h.announce).toHaveBeenCalledWith('Period of 4');
  });

  it('is said in a widget with nothing to read', async () => {
    await renderView(false);
    act(() => screen.getByRole('grid').focus());
    turn(7);
    expect(h.announce).toHaveBeenCalledWith('Period of 11');
  });

  it('is nothing to say when the anchor moved without one: a step in a grid', async () => {
    await renderView();
    act(() => screen.getByRole('button').focus());
    act(() => h.set({ anchor: new Date(2026, 10, 5) }));
    expect(h.announce).not.toHaveBeenCalled();
  });
});
