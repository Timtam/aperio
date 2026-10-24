import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Checking whether the attendees are free (decisions 270, 273): a check the
 * provider turned away because it is busy says so, and the answer of an
 * earlier check goes — "All attendees are free" must never stand beside a
 * check that could not be made.
 */

const { invokeMock, answers } = vi.hoisted(() => {
  const answers: { next: () => Promise<unknown> } = { next: () => Promise.resolve([]) };
  const invokeMock = vi.fn((command: string) => {
    if (command === 'query_free_busy') return answers.next();
    if (command === 'calendar_current_user_email') return Promise.resolve('me@example.com');
    if (command === 'get_user_pref') return Promise.resolve(null);
    return Promise.resolve([]);
  });
  return { invokeMock, answers };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const WORK = {
  id: 'cal-work',
  name: 'Arbeit',
  read_only: false,
  account_id: 'acc-work',
  supports_scheduling: true,
  always_notifies_attendees: false,
  invitations_reply_only: false,
  notifier_name: null,
} as unknown as Calendar;

const MEETING = {
  id: 'ev-1',
  calendar_id: 'cal-work',
  title: 'Planung',
  description: null,
  location: null,
  start: '2026-11-09T15:00:00.000Z',
  end: '2026-11-09T15:30:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: ['Kim <kim@example.net>'],
  organizer: 'me@example.com',
  organized_elsewhere: false,
  attendee_responses: [],
} as unknown as CalendarEvent;

const STORE = {
  calendars: [WORK] as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {}, invalidateData: () => {} };
const REMINDERS = { getDefaultsFor: () => [] };

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => REMINDERS,
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

describe('EventDialog → checking availability', () => {
  it('says a busy provider, and the earlier "all free" goes', async () => {
    const { EventDialog } = await import('./EventDialog');
    render(<EventDialog isOpen onClose={() => {}} event={MEETING} />);
    const check = await screen.findByRole(
      'button',
      { name: /verfügbarkeit prüfen|check availability/i },
      { timeout: 8000 },
    );

    answers.next = () => Promise.resolve([]);
    fireEvent.click(check);
    await screen.findByText(/alle teilnehmer sind zu dieser zeit frei/i);

    answers.next = () =>
      Promise.reject({ code: 'forbidden', message: 'provider-busy: http-429' });
    fireEvent.click(check);
    await screen.findByText(
      'Die Verfügbarkeit ließ sich nicht prüfen: der Anbieter ist gerade ausgelastet. ' +
        'Versuche es später noch einmal.',
    );
    expect(screen.queryByText(/alle teilnehmer sind zu dieser zeit frei/i)).toBeNull();
    expect(screen.queryByText(/nichts geändert|provider-busy/i)).toBeNull();
  });
});
