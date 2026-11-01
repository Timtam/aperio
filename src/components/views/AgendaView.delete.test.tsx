import { afterEach, describe, expect, it, vi } from 'vitest';
import { markShownInNotice } from '@aperio/shared';
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
const VIEW_STATE = { anchor: new Date(2026, 5, 15), weekStartsOn: 1, weekNumbers: true };
/** How often the view asked for its data to be read again. */
let invalidations = 0;
/** What the "please wait" dialog said where a write failed (decision 210). */
const noticed: string[] = [];
/** Whether the dialog's wait marks a failure as said, as DialogState does. */
let waitSaysFailures = false;
const DIALOG_STATE = {
  openEventDialog: () => {},
  openMoveCopy: () => {},
  invalidateData: () => {
    invalidations += 1;
  },
  dataVersion: 0,
  stepWriteWait:
    (failure: (err: unknown) => string) =>
    async <T,>(_inSteps: boolean, write: () => Promise<T>): Promise<T> => {
      try {
        return await write();
      } catch (err) {
        if (!waitSaysFailures) throw err;
        noticed.push(failure(err));
        throw markShownInNotice(err);
      }
    },
};
const MENU = { openForEvent: () => {} };
/** What the view announced, in order. */
const announced: string[] = [];
const announce = (message: string) => {
  announced.push(message);
};

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
  invokeMock.mockImplementation(() => Promise.resolve([] as unknown));
  announced.length = 0;
  noticed.length = 0;
  waitSaysFailures = false;
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

  it('says a refused delete in words, as the editors do, not as its token', async () => {
    invokeMock.mockImplementation((command: string) =>
      command === 'delete_event'
        ? Promise.reject({ code: 'forbidden', message: 'day-zone-unreadable: no-zone' })
        : Promise.resolve([] as unknown),
    );
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'Delete' });
    fireEvent.click(
      await screen.findByRole('button', {
        name: /absagen & teilnehmer benachrichtigen|cancel & notify attendees/i,
      }),
    );
    await waitFor(() => expect(announced.length).toBeGreaterThan(0));
    expect(announced.at(-1)).toMatch(
      /Zeitzone, die Aperio nicht lesen kann|time zone Aperio cannot read/,
    );
    expect(announced.join(' ')).not.toMatch(/day-zone-unreadable|forbidden:/);
  });

  it('reads the list again after a delete that failed, which may have landed in part', async () => {
    invokeMock.mockImplementation((command: string) =>
      command === 'delete_event'
        ? Promise.reject({ code: 'network', message: 'connection reset' })
        : Promise.resolve([] as unknown),
    );
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    const before = invalidations;
    fireEvent.keyDown(list, { key: 'Delete' });
    fireEvent.click(
      await screen.findByRole('button', {
        name: /absagen & teilnehmer benachrichtigen|cancel & notify attendees/i,
      }),
    );
    await waitFor(() => expect(announced.length).toBeGreaterThan(0));
    expect(invalidations).toBeGreaterThan(before);
  });
});

describe('AgendaView → Ctrl+D', () => {
  it('says the duplicate and reads the calendar again, so it shows', async () => {
    invalidations = 0;
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'd', ctrlKey: true });
    await waitFor(() => expect(announced.length).toBeGreaterThan(0));
    expect(announced.at(-1)).toMatch(/„Planung" wurde dupliziert|"Planung" duplicated/);
    expect(invalidations).toBeGreaterThan(0);
  });
});

describe('AgendaView → Ctrl+D that fails', () => {
  it('says it in words, with the reason (decision 261)', async () => {
    invokeMock.mockImplementation((command: string) =>
      command === 'create_event'
        ? Promise.reject(new Error('Der Server antwortet nicht'))
        : Promise.resolve([] as unknown),
    );
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'd', ctrlKey: true });
    await waitFor(() => expect(announced.length).toBeGreaterThan(0));
    expect(announced.at(-1)).toMatch(/konnte nicht dupliziert werden|could not be duplicated/);
    expect(announced.at(-1)).toMatch(/Der Server antwortet nicht/);
  });

  it('says a copy that may stand, not to repeat it, and reads the calendar again (210)', async () => {
    invalidations = 0;
    invokeMock.mockImplementation((command: string) =>
      command === 'create_event'
        ? Promise.reject({ code: 'network', message: 'created-may-stand: no-occurrence' })
        : Promise.resolve([] as unknown),
    );
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'd', ctrlKey: true });
    await waitFor(() => expect(announced.length).toBeGreaterThan(0));
    expect(announced.at(-1)).toMatch(/möglicherweise doppelt im Kalender|may now be in the calendar twice/);
    expect(announced.at(-1)).toMatch(/wiederhole es nicht|do not repeat/);
    expect(invalidations).toBeGreaterThan(0);
  });

  it('says a failure once: not again where the waiting dialog said it', async () => {
    waitSaysFailures = true;
    invokeMock.mockImplementation((command: string) =>
      command === 'create_event'
        ? Promise.reject({ code: 'forbidden', message: 'deletions-not-kept: no-occurrence' })
        : Promise.resolve([] as unknown),
    );
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'd', ctrlKey: true });
    await waitFor(() => expect(noticed.length).toBe(1));
    expect(noticed[0]).toMatch(/konnte nicht dupliziert werden|could not be duplicated/);
    // A tick for anything the view would say after it.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(announced).toHaveLength(0);
    waitSaysFailures = false;
  });

  it('duplicates once while a Ctrl+D is on its way', async () => {
    let answer = () => {};
    invokeMock.mockImplementation((command: string) =>
      command === 'create_event'
        ? new Promise((resolve) => {
            answer = () => resolve({ ...MEETING, id: 'copy' });
          })
        : Promise.resolve([] as unknown),
    );
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Planung/);
    fireEvent.keyDown(list, { key: 'd', ctrlKey: true });
    fireEvent.keyDown(list, { key: 'd', ctrlKey: true });
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter((c) => c[0] === 'create_event')).toHaveLength(1),
    );
    answer();
    await waitFor(() => expect(announced.length).toBeGreaterThan(0));
    expect(invokeMock.mock.calls.filter((c) => c[0] === 'create_event')).toHaveLength(1);
  });
});
