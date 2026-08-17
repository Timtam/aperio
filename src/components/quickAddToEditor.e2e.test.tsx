import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * The whole hand-off, end to end: quick-add → accept an offer → the editor.
 *
 * Both halves pass in isolation, which is exactly why this exists. The report
 * is about what the user actually does, and that runs through the dialog
 * stack, `replaceTop`, and DialogHost swapping one component for another.
 */

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-local', name: 'Kalender', read_only: false } as unknown as Calendar,
  { id: 'cal-work', name: 'Arbeit', read_only: false } as unknown as Calendar,
];

const SOURCE: CalendarEvent = {
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

const STORE = {
  calendars: CALENDARS,
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local', 'cal-work']),
};
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};
const REMINDERS = { getDefaultsFor: () => [] };

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => REMINDERS,
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [SOURCE] };
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('quick-add → accept an offer → editor', () => {
  it("opens the editor on the offer's calendar", async () => {
    const { DialogStateProvider } = await import('../state/DialogState');
    const { useDialogState } = await import('../state/dialogStateContext');
    const { DialogHost } = await import('./DialogHost');

    function OpenQuickAdd() {
      const { openQuickAdd } = useDialogState();
      return (
        <button type="button" onClick={() => openQuickAdd({})}>
          open
        </button>
      );
    }

    // StrictMode as in src/main.tsx. The hand-off MOUNTS the editor, and a
    // mount is exactly when React double-invokes passive effects — which is
    // what made this test green while the real app was broken.
    render(
      <StrictMode>
        <DialogStateProvider>
          <OpenQuickAdd />
          <DialogHost />
        </DialogStateProvider>
      </StrictMode>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'open' }));

    const title = await screen.findByRole('combobox', { name: /titel/i });
    fireEvent.change(title, { target: { value: 'thomas' } });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'Enter' });

    const picker = (await screen.findByRole('combobox', {
      name: /kalender/i,
    })) as HTMLSelectElement;
    await waitFor(() => expect(picker.value).toBe('cal-work'));
  });
});
