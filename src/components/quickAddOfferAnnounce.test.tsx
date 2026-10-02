import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * What the screen reader is actually left with after the quick-add hands an
 * offer to the editor — through the REAL announcer, whose live region keeps
 * only the last of two announcements made in one frame.
 *
 * The quick-add says "filled in" and the editor, opening in the same task,
 * says why it keeps its own calendar. Said separately, the second replaced
 * the first and the fill went unmentioned.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) => {
    if (command === 'get_user_pref') return Promise.resolve(null);
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS = [
  { id: 'cal-local', name: 'Kalender', read_only: false },
  { id: 'cal-feed', name: 'Abo', read_only: true },
] as unknown as Calendar[];
const OFFER = {
  id: 'feed-1',
  calendar_id: 'cal-feed',
  title: 'Thomas Meeting',
  description: 'Agenda',
  location: null,
  start: '2026-06-15T09:00:00.000Z',
  end: '2026-06-15T09:45:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

const STORE = {
  calendars: CALENDARS,
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local', 'cal-feed']),
};
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [OFFER] };
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('quick-add → editor, through the real announcer', () => {
  it('leaves the fill AND the refusal in the live region', async () => {
    const { AnnouncerProvider } = await import('../a11y/Announcer');
    const { DialogStateProvider } = await import('../state/DialogState');
    const { useDialogState } = await import('../state/dialogStateContext');
    const { DialogHost } = await import('./DialogHost');

    function Opener() {
      const { openQuickAdd } = useDialogState();
      return (
        <button type="button" onClick={() => openQuickAdd({})}>
          open
        </button>
      );
    }
    render(
      <AnnouncerProvider>
        <DialogStateProvider>
          <Opener />
          <DialogHost />
        </DialogStateProvider>
      </AnnouncerProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'open' }));

    const title = await screen.findByRole('combobox', { name: /titel/i });
    fireEvent.change(title, { target: { value: 'thomas' } });
    await screen.findByRole('option', { name: /thomas/i });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'Enter' });
    await screen.findByRole('combobox', { name: /kalender/i });
    await act(() => new Promise((resolve) => setTimeout(resolve, 200)));

    expect(screen.getByTestId('announcer-polite').textContent).toBe(
      'Aus „Thomas Meeting" übernommen. Der Tag bleibt, wie er war. ' +
        '„Abo“ nimmt keine neuen Termine an. Der Termin kommt in „Kalender“.',
    );
  }, 15_000);
});
