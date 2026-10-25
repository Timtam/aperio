import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent, ImportedEvent } from '../api/types';

/**
 * An appointment read from a file fills the editor as it stands
 * (DESIGN-file-import.md): its day and time — unlike a title suggestion,
 * whose day stays the one the editor opened on —, its series with the
 * file's gaps and zone, and the import's note as the first stop. The
 * calendar is the editor's own default: the picker is where it goes.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
    if (command === 'create_event') {
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'created-1' } as unknown as CalendarEvent);
    }
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-local', name: 'Kalender', read_only: false, account_id: 'local' } as unknown as Calendar,
];
const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local']),
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
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<typeof import('../state/useTitleSuggestions')>(
    '../state/useTitleSuggestions',
  );
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

const SERIES: ImportedEvent = {
  uid: 'teams-series-0001@example.org',
  title: 'Jour fixe',
  description: 'Teams-Besprechung',
  location: 'Raum 3.14',
  start: '2026-11-02T09:00:00Z',
  end: '2026-11-02T09:30:00Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO',
    exceptions: ['2026-11-09T09:00:00Z', '2026-11-16T09:00:00Z'],
    tzid: 'Europe/Berlin',
  },
  reminders: [{ kind: { type: 'relative', minutes_before: 15 }, sound: null }],
  attendees: [],
};

async function openImport(onImported = () => {}) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog
        isOpen
        onClose={() => {}}
        event={null}
        importFrom={SERIES}
        importNote="Nicht übernommen: die Aufgabe „Geschenke besorgen“."
        onImported={onImported}
      />
    </StrictMode>,
  );
}

describe('EventDialog → an appointment from a file', () => {
  it('opens on the file’s day and says what was left out first', async () => {
    await openImport();
    expect(screen.getByRole('dialog', { name: 'Termin aus Datei' })).toBeTruthy();
    expect(
      screen.getByText('Nicht übernommen: die Aufgabe „Geschenke besorgen“.'),
    ).toBeTruthy();
    const title = screen.getByRole('combobox', { name: /^titel$/i }) as HTMLInputElement;
    expect(title.value).toBe('Jour fixe');
    const startDate = new Date(SERIES.start);
    const pad = (n: number) => String(n).padStart(2, '0');
    const day = `${startDate.getFullYear()}-${pad(startDate.getMonth() + 1)}-${pad(startDate.getDate())}`;
    expect(screen.getAllByDisplayValue(day).length).toBeGreaterThan(0);
    expect(screen.getByDisplayValue('Raum 3.14')).toBeTruthy();
  });

  it('creates the series with the file’s gaps and zone, and says so', async () => {
    const onImported = vi.fn();
    await openImport(onImported);
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    const create = invokeMock.mock.calls.find(([command]) => command === 'create_event');
    const request = (create?.[1] as { request: Record<string, unknown> }).request;
    expect(request.recurrence).toEqual(SERIES.recurrence);
    expect(request.start).toBe('2026-11-02T09:00:00.000Z');
    expect(request.calendar_id).toBe('cal-local');
    expect(request.reminders).toEqual(SERIES.reminders);
    await waitFor(() => expect(onImported).toHaveBeenCalledTimes(1));
  });

  it('gives a series the file keeps in UTC no zone of the device', async () => {
    const { EventDialog } = await import('./EventDialog');
    const utcSeries: ImportedEvent = {
      ...SERIES,
      recurrence: { rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] },
    };
    render(<EventDialog isOpen onClose={() => {}} event={null} importFrom={utcSeries} />);
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    const create = invokeMock.mock.calls.find(([command]) => command === 'create_event');
    const request = (create?.[1] as { request: { recurrence: Record<string, unknown> } }).request;
    expect(request.recurrence).toEqual({ rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] });
  });
});
