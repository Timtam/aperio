import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * A whole series saved into another calendar is written there as a NEW
 * series (decisions 224, 259): with its real deletions only. An occurrence
 * changed elsewhere — Exchange lists its slot among the exceptions and keeps
 * its row — comes back at its usual time instead of travelling as a deletion.
 * The same save in its own calendar leaves the exceptions as they are.
 */

const { invokeMock, rowsFail } = vi.hoisted(() => {
  const rowsFail = { current: false };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'get_series_rows') {
      if (rowsFail.current) return Promise.reject({ code: 'network', message: 'offline' });
      return Promise.resolve({ rows: [CHANGED], reach: { kind: 'complete' } });
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, rowsFail };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'acc-ews' } as unknown as Calendar,
  { id: 'cal-home', name: 'Privat', read_only: false, account_id: 'acc-icloud' } as unknown as Calendar,
];

/** Mondays at 09:00 UTC from 2 November, four times: the 9 November deleted,
 *  the 16 November changed in Outlook. */
const SERIES: CalendarEvent = {
  id: 'M:series|CK',
  calendar_id: 'cal-work',
  title: 'Teamrunde',
  description: null,
  location: null,
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T10:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO;COUNT=4',
    exceptions: ['2026-11-09T09:00:00.000Z', '2026-11-16T09:00:00.000Z'],
    tzid: 'Europe/Berlin',
  },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;
/** The row Exchange keeps for the changed 16 November, moved to 11:00. */
const CHANGED = {
  ...SERIES,
  id: 'M:series|CK::rid::2026-11-16T09:00:00.000Z',
  start: '2026-11-16T11:00:00.000Z',
  end: '2026-11-16T12:00:00.000Z',
  recurrence: null,
} as unknown as CalendarEvent;

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work', 'cal-home']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {} };
const REMINDERS = { getDefaultsFor: () => [] };
const announce = () => {};

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => REMINDERS,
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<typeof import('../state/useTitleSuggestions')>(
    '../state/useTitleSuggestions',
  );
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  rowsFail.current = false;
  vi.restoreAllMocks();
});

function updatesSent(): { event: CalendarEvent; previousCalendarId: string | null }[] {
  return invokeMock.mock.calls
    .filter((call) => call[0] === 'update_event')
    .map((call) => call[1] as { event: CalendarEvent; previousCalendarId: string | null });
}

async function open() {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={SERIES} />
    </StrictMode>,
  );
  return screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

describe('EventDialog → a whole series saved into another calendar (224, 259)', () => {
  it('carries the real deletions only: the changed occurrence comes back', async () => {
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    const [{ event, previousCalendarId }] = updatesSent();
    expect(event.calendar_id).toBe('cal-home');
    expect(previousCalendarId).toBe('cal-work');
    expect(event.recurrence?.exceptions).toEqual(['2026-11-09T09:00:00.000Z']);
    expect(event.recurrence?.tzid).toBe('Europe/Berlin');
  });

  it('writes nothing when the series rows cannot be read', async () => {
    rowsFail.current = true;
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'get_series_rows')).toBe(true),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(updatesSent()).toHaveLength(0);
  });

  it('leaves the exceptions as they are when the series stays in its calendar', async () => {
    await open();
    fireEvent.change(screen.getByRole('combobox', { name: /titel|title/i }), {
      target: { value: 'Teamrunde neu' },
    });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    expect(updatesSent()[0].event.recurrence?.exceptions).toEqual([
      '2026-11-09T09:00:00.000Z',
      '2026-11-16T09:00:00.000Z',
    ]);
  });
});
