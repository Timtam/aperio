import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar } from '../api/types';

/**
 * What the quick-add says when the new appointment could not be saved
 * (decision 273): a busy provider in words, and an appointment that stands
 * without one of its steps on screen with only Close — saving again would
 * create it twice.
 */

const { invokeMock, failure, invalidateData } = vi.hoisted(() => {
  const failure: { next: unknown } = { next: null };
  const invokeMock = vi.fn((command: string) =>
    command === 'create_event' && failure.next != null
      ? Promise.reject(failure.next)
      : Promise.resolve({ id: 'ev-1' }),
  );
  return { invokeMock, failure, invalidateData: vi.fn() };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const STORE = {
  calendars: [{ id: 'cal-work', name: 'Arbeit', read_only: false }] as unknown as Calendar[],
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ openEventDialog: () => {}, invalidateData }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<typeof import('../state/useTitleSuggestions')>(
    '../state/useTitleSuggestions',
  );
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  invalidateData.mockClear();
  failure.next = null;
});

async function createAppointment(onClose = () => {}) {
  const { QuickAddDialog } = await import('./QuickAddDialog');
  render(<QuickAddDialog isOpen onClose={onClose} />);
  fireEvent.change(screen.getByRole('combobox', { name: /titel/i }), {
    target: { value: 'Zahnarzt' },
  });
  fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
}

describe('QuickAddDialog → a create that failed', () => {
  it('says a busy provider in words', async () => {
    failure.next = { code: 'forbidden', message: 'provider-busy: http-429' };
    await createAppointment();
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Der Anbieter ist gerade ausgelastet. Es wurde nichts geändert. Versuche es später noch einmal.',
    );
  });

  it('only closes once the appointment stands without one of its steps', async () => {
    failure.next = { code: 'network', message: 'partly-saved: extension' };
    const onClose = vi.fn();
    await createAppointment(onClose);
    const note = await screen.findByText(/nur zum Teil gespeichert/);
    await waitFor(() => expect(document.activeElement).toBe(note));
    expect(screen.queryByRole('button', { name: /^anlegen$/i })).toBeNull();
    expect(invalidateData).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /^schließen$/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
