import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar } from '../api/types';

/**
 * The quick-add makes no reminder choice — it has no reminder field at all —
 * so it has to say so, and only then may the host write the calendar's
 * attached default into the new appointment.
 *
 * Nothing pinned this. The flag is optional on all four sides (`#[serde(default)]`
 * in both hosts, an optional field in both TypeScript request types), so
 * dropping it looks exactly like declining it: the appointment saves, the
 * dialog closes, and the alarm the user configured never reaches the provider.
 * The only thing that would say otherwise is a phone that stays quiet.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
    void command;
    void payload;
    return Promise.resolve({ id: 'ev-1' });
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const CALENDARS: Calendar[] = [
  { id: 'cal-work', name: 'Arbeit', read_only: false } as unknown as Calendar,
];

const STORE = {
  calendars: CALENDARS,
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ openEventDialog: () => {} }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

describe('QuickAddDialog → creating an appointment', () => {
  it('tells the host no reminder choice was made', async () => {
    const { QuickAddDialog } = await import('./QuickAddDialog');
    render(<QuickAddDialog isOpen onClose={() => {}} />);

    const title = screen.getByRole('combobox', { name: /titel/i });
    fireEvent.change(title, { target: { value: 'Zahnarzt' } });
    screen.getByRole('button', { name: /^anlegen$|^create$/i }).click();

    await waitFor(() =>
      expect(invokeMock.mock.calls.some((c) => c[0] === 'create_event')).toBe(true),
    );
    const create = invokeMock.mock.calls.find((c) => c[0] === 'create_event');
    const payload = (
      create?.[1] as {
        request: { reminders: unknown[]; use_calendar_defaults?: boolean };
      }
    ).request;
    // No reminder field here at all, so an empty list is an omission rather
    // than a decision — and the flag is what tells the host which it is.
    expect(payload.reminders).toEqual([]);
    expect(payload.use_calendar_defaults).toBe(true);
  });
});
