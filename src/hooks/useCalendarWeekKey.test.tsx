import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ViewId, WeekStart } from '../state/viewMath';

/**
 * The K key says the calendar week of the day the view stands on (TODO B15),
 * in the calendar view only: the sidebar finds entries by their first
 * letters, a text field takes the letter as typed, and the backlog stands on
 * no day.
 */

const h = vi.hoisted(() => ({
  viewState: {
    view: 'week' as ViewId,
    anchor: new Date(2026, 10, 4),
    weekStartsOn: 1 as WeekStart,
    weekNumbers: true,
  },
  announce: vi.fn(),
}));

vi.mock('../state/viewStateContext', () => ({ useViewState: () => h.viewState }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));

beforeEach(() => {
  h.viewState.view = 'week';
  h.viewState.anchor = new Date(2026, 10, 4);
  h.viewState.weekStartsOn = 1;
  h.viewState.weekNumbers = true;
  h.announce.mockReset();
});

afterEach(() => {
  document.body.innerHTML = '';
});

async function renderShell() {
  const { useCalendarWeekKey } = await import('./useCalendarWeekKey');
  function Shell() {
    useCalendarWeekKey();
    return (
      <>
        <div data-region="sidebar">
          <button type="button">Kalender</button>
        </div>
        <div data-region="view">
          <div role="grid" tabIndex={0} aria-label="Wochenraster" />
          <input aria-label="Suche" />
          <div data-region="backlog">
            <button type="button">Backlog</button>
          </div>
        </div>
      </>
    );
  }
  render(<Shell />);
}

describe('the K key', () => {
  it('in the calendar view says the week with its days', async () => {
    await renderShell();
    fireEvent.keyDown(screen.getByRole('grid'), { key: 'k' });
    expect(h.announce).toHaveBeenCalledWith('Kalenderwoche 45, 2. bis 8. November 2026');
  });

  it('answers with the week start the views use', async () => {
    h.viewState.weekStartsOn = 0;
    h.viewState.anchor = new Date(2026, 10, 1);
    await renderShell();
    fireEvent.keyDown(screen.getByRole('grid'), { key: 'K' });
    expect(h.announce).toHaveBeenCalledWith('Kalenderwoche 45, 1. bis 7. November 2026');
  });

  it('answers in every view that stands on a day, and with weeks switched off', async () => {
    h.viewState.view = 'agenda';
    h.viewState.weekNumbers = false;
    await renderShell();
    fireEvent.keyDown(screen.getByRole('grid'), { key: 'k' });
    expect(h.announce).toHaveBeenCalledTimes(1);
  });

  it('leaves the letter to the sidebar and text fields, and the backlog alone', async () => {
    await renderShell();
    fireEvent.keyDown(screen.getByRole('button', { name: 'Kalender' }), { key: 'k' });
    fireEvent.keyDown(screen.getByRole('button', { name: 'Backlog' }), { key: 'k' });
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Suche' }), { key: 'k' });
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('is no shortcut with a modifier, and none in the task and contact views', async () => {
    await renderShell();
    fireEvent.keyDown(screen.getByRole('grid'), { key: 'k', ctrlKey: true });
    fireEvent.keyDown(screen.getByRole('grid'), { key: 'k', altKey: true });
    expect(h.announce).not.toHaveBeenCalled();
    cleanup();
    h.viewState.view = 'tasks';
    await renderShell();
    fireEvent.keyDown(screen.getByRole('grid'), { key: 'k' });
    expect(h.announce).not.toHaveBeenCalled();
  });
});
