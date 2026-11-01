import { act, render, screen } from '@testing-library/react';
import { useRef } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { PageTurnOptions } from './usePageTurnAnnouncement';

/**
 * A page turn (Back, Today, Forward, Ctrl+←/→, Ctrl+T) is said unless what
 * has the focus says it (TODO B15 step 3). NVDA reads the name of what has
 * the focus when it changes, so the view stays quiet exactly when the turn
 * renamed it, and says the period whenever nothing was renamed.
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

/**
 * A view whose active entry is named after the anchor's day — or, with
 * `entryFor` returning null, has no entry — and a check-in button outside
 * the widget, named after the day as well.
 */
async function renderView(
  options?: PageTurnOptions,
  entryFor: (day: number) => string | null = (day) => `Entry on ${day}`,
) {
  const { usePageTurnAnnouncement } = await import('./usePageTurnAnnouncement');
  function View() {
    const widget = useRef<HTMLDivElement>(null);
    const { anchor } = h.get();
    const day = anchor.getDate();
    const entry = entryFor(day);
    usePageTurnAnnouncement(`Period of ${day}`, widget, options);
    return (
      <>
        <button type="button">Forward</button>
        <button type="button" aria-label={`Check-in: ${day}`} />
        <div
          ref={widget}
          role="listbox"
          tabIndex={0}
          aria-label="List"
          aria-activedescendant={entry ? 'entry' : undefined}
        >
          {entry && <div id="entry" role="option" aria-label={entry} aria-selected />}
        </div>
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
  it('is said with the focus on a toolbar button', async () => {
    await renderView();
    act(() => screen.getByRole('button', { name: 'Forward' }).focus());
    turn(7);
    expect(h.announce).toHaveBeenCalledWith('Period of 11');
  });

  it('is left to the widget when it renamed its active entry', async () => {
    await renderView();
    act(() => screen.getByRole('listbox').focus());
    turn(7);
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('is said in the widget when the active entry stays as it was', async () => {
    // The agenda: a few days on, the same meeting is still the first entry.
    await renderView(undefined, () => 'Wednesday 18, Review');
    act(() => screen.getByRole('listbox').focus());
    turn(-3);
    expect(h.announce).toHaveBeenCalledWith('Period of 1');
  });

  it('is said in the widget for Today on today, which renames nothing', async () => {
    await renderView();
    act(() => screen.getByRole('listbox').focus());
    turn(0);
    expect(h.announce).toHaveBeenCalledWith('Period of 4');
  });

  it('is said in a widget with no entry', async () => {
    await renderView(undefined, () => null);
    act(() => screen.getByRole('listbox').focus());
    turn(7);
    expect(h.announce).toHaveBeenCalledWith('Period of 11');
  });

  it('is said when the widget lost its entry: an empty day after a full one', async () => {
    await renderView(undefined, (day) => (day === 4 ? 'Entry on 4' : null));
    act(() => screen.getByRole('listbox').focus());
    turn(7);
    expect(h.announce).toHaveBeenCalledWith('Period of 11');
  });

  it('is left to the view while the widget is still loading its entries', async () => {
    await renderView({ pending: true }, () => null);
    act(() => screen.getByRole('listbox').focus());
    turn(7);
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('is compared with where the focus went, not with the last render', async () => {
    // Tab from the toolbar into the list renders nothing.
    await renderView();
    act(() => screen.getByRole('button', { name: 'Forward' }).focus());
    act(() => screen.getByRole('listbox').focus());
    turn(7);
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('on a renamed check-in button: said, unless the view says that is enough', async () => {
    await renderView();
    act(() => screen.getByRole('button', { name: 'Check-in: 4' }).focus());
    turn(7);
    expect(h.announce).toHaveBeenCalledWith('Period of 11');
  });

  it('on a renamed check-in button that names the period: left to it', async () => {
    await renderView({ focusNameSays: true });
    act(() => screen.getByRole('button', { name: 'Check-in: 4' }).focus());
    turn(7);
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('is nothing to say when the anchor moved without one: a step in a grid', async () => {
    await renderView();
    act(() => screen.getByRole('button', { name: 'Forward' }).focus());
    act(() => h.set({ anchor: new Date(2026, 10, 5) }));
    expect(h.announce).not.toHaveBeenCalled();
  });
});
