import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { CalendarEvent } from '../api/types';

/**
 * Coming back from a hit you just edited.
 *
 * Opening a result takes this dialog down and puts the editor in its place;
 * closing the editor mounts a FRESH search dialog. Toni's report: the query
 * and every result were gone, so the search had to be typed again after each
 * edit — worst for someone who navigates by keyboard and hears the list.
 *
 * The query and the filters survive; the RESULTS are re-fetched, so what comes
 * back reflects the edit that was just made. Closing the dialog on purpose is
 * the end of searching and clears it.
 */

const HIT: CalendarEvent = {
  id: 'ev-thomas',
  calendar_id: 'cal-work',
  title: 'Thomas Meeting',
  description: null,
  location: null,
  start: '2026-06-15T09:00:00.000Z',
  end: '2026-06-15T10:00:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) => {
    if (command === 'search') {
      return Promise.resolve({ events: [HIT], tasks: [] });
    }
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const STORE = {
  calendars: [
    { id: 'cal-work', name: 'Arbeit', read_only: false },
  ] as unknown as [],
  taskLists: [] as unknown as [],
};

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({
    openEventDialog: () => {},
    openTaskDialog: () => {},
  }),
}));

afterEach(() => {
  document.body.innerHTML = '';
});

/** The search is debounced 200 ms; the rest of the window is for the full
 *  suite, where the 1 s default expires under parallel load. */
const APPEARS = { timeout: 8000 };

async function searchFor(term: string) {
  const { SearchDialog } = await import('./SearchDialog');
  const view = render(<SearchDialog isOpen onClose={() => {}} />);
  const field = screen.getByRole('searchbox');
  fireEvent.change(field, { target: { value: term } });
  await screen.findByText(/Thomas Meeting/, {}, APPEARS);
  return view;
}

describe('SearchDialog → coming back after editing a hit', () => {
  it('brings the query and the results back', async () => {
    const { unmount } = await searchFor('thomas');
    // Opening a hit takes this dialog down; closing the editor brings a new
    // one up. Nobody CLOSED the search.
    unmount();

    const { SearchDialog } = await import('./SearchDialog');
    render(<SearchDialog isOpen onClose={() => {}} />);
    expect(screen.getByRole('searchbox')).toHaveValue('thomas');
    // Re-fetched, not restored from a stale list: the row the user just
    // edited has to come back as it is NOW.
    await screen.findByText(/Thomas Meeting/, {}, APPEARS);
  }, 20_000);

  it('starts clean once the user closes it', async () => {
    const { unmount } = await searchFor('thomas');
    // The dialog offers two ways out (the header's × and the Close button);
    // both go through the same handler, and both mean "done searching".
    const closers = screen.getAllByRole('button', { name: /schließen|close/i });
    fireEvent.click(closers[closers.length - 1]);
    unmount();

    const { SearchDialog } = await import('./SearchDialog');
    render(<SearchDialog isOpen onClose={() => {}} />);
    expect(screen.getByRole('searchbox')).toHaveValue('');
  }, 20_000);
});
