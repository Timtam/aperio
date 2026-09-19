import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../../api/types';

/**
 * The Delete key in a view asks what the editor asks (decision 80a). On a
 * calendar whose provider cancels a meeting for its attendees whatever the
 * request says (iCloud, Microsoft 365), a one-off meeting the account
 * organizes used to take a plain "Delete?" and go out as a silent delete: the
 * guests got the cancellation, and the user never heard that they would.
 * WeekView, DayView and MonthView share the same `DeleteEventConfirm`.
 */

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn((_command: string, _payload?: unknown) => Promise.resolve([] as unknown)),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const ICLOUD = {
  id: 'cal-icloud',
  name: 'iCloud',
  read_only: false,
  supports_scheduling: true,
  always_notifies_attendees: true,
  notifier_name: 'iCloud',
} as unknown as Calendar;

const MEETING = {
  id: 'ev-1',
  calendar_id: 'cal-icloud',
  title: 'Planung',
  description: null,
  location: null,
  start: new Date(2026, 5, 15, 10, 0).toISOString(),
  end: new Date(2026, 5, 15, 11, 0).toISOString(),
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: ['bob@example.com'],
  organizer: 'me@example.com',
  organized_elsewhere: false,
} as unknown as CalendarEvent;

// Stable identities: a fresh array per render would re-derive the rows forever.
const EVENTS = [MEETING];
const CALENDAR_BY_ID = new Map([[ICLOUD.id, ICLOUD]]);
const GROUPS: unknown[] = [];
const STORE = {
  calendars: [ICLOUD],
  colorLabels: [],
  selectedCalendarIds: new Set([ICLOUD.id]),
};
const VIEW_STATE = { anchor: new Date(2026, 5, 15) };
const DIALOG_STATE = {
  openEventDialog: () => {},
  openMoveCopy: () => {},
  invalidateData: () => {},
  dataVersion: 0,
};
const MENU = { openForEvent: () => {} };
const announce = () => {};

vi.mock('../../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../../state/useEvents', () => ({
  useEvents: () => ({ events: EVENTS, calendarById: CALENDAR_BY_ID, loading: false }),
}));
vi.mock('../../state/useEventGroups', () => ({
  useEventGroups: (events: CalendarEvent[]) => ({ groups: GROUPS, events }),
}));
vi.mock('../../state/useChipContextMenu', () => ({ useChipContextMenu: () => MENU }));
vi.mock('../../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../DayCheckInButton', () => ({ DayCheckInButton: () => null }));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

describe('AgendaView → the Delete key on a meeting the provider always cancels', () => {
  it('says who informs the attendees and deletes with the cancellation', async () => {
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'Delete' });
    await screen.findByText(
      /iCloud informiert die Teilnehmer über die Absage|iCloud informs the attendees of the cancellation/i,
    );
    fireEvent.click(
      screen.getByRole('button', { name: /absagen & teilnehmer benachrichtigen|cancel & notify attendees/i }),
    );
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'delete_event')).toBe(true),
    );
    const del = invokeMock.mock.calls.find((call) => call[0] === 'delete_event');
    expect(del?.[1]).toMatchObject({ id: 'ev-1', sendCancellations: true });
  });
});
