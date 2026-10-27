import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent, ImportedEvent } from '../api/types';

/**
 * An appointment read from a file fills the editor as it stands
 * (DESIGN-file-import.md): its day and time — unlike a title suggestion,
 * whose day stays the one the editor opened on —, its series with the
 * file's gaps and zone, and the import's note as the first stop. The
 * calendar is the editor's own default: the picker is where it goes.
 */

/** What the calendar holds when asked (`get_events`), or an answer that does
 *  not come. */
const calendarRows = vi.hoisted(() => ({
  current: [] as Array<{ ical_uid?: string }> | 'pending',
}));
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
    if (command === 'get_events') {
      const rows = calendarRows.current;
      return rows === 'pending' ? new Promise(() => {}) : Promise.resolve(rows);
    }
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
  {
    id: 'cal-local',
    name: 'Kalender',
    read_only: false,
    account_id: 'local',
    keeps_uid: true,
  } as unknown as Calendar,
];
const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local']) as ReadonlySet<string>,
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {}, invalidateData: () => {} };
const REMINDERS = { getDefaultsFor: () => [] };

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
/** The account's address in the calendar, an answer that does not come, or
 *  one the test gives when it chooses (`answerOwn`). */
const ownAddress = vi.hoisted(() => ({
  current: null as string | null | 'pending' | 'later',
  answer: null as null | ((address: string | null) => void),
}));
vi.mock('../state/currentUserEmail', () => ({
  resolveCalendarUserEmail: () =>
    ownAddress.current === 'pending'
      ? new Promise(() => {})
      : ownAddress.current === 'later'
        ? new Promise((resolve) => {
            ownAddress.answer = resolve;
          })
        : Promise.resolve(ownAddress.current),
}));
/** The signature bound to the calendar, the same object on every read. */
const boundSignature = vi.hoisted(() => ({
  current: null as null | { id: string; name: string; body: string },
  calendarId: 'cal-local',
}));
vi.mock('../state/useSignatures', async () => {
  const actual = await vi.importActual<typeof import('../state/useSignatures')>(
    '../state/useSignatures',
  );
  return {
    ...actual,
    useSignatures: () => ({
      signatures: boundSignature.current ? [boundSignature.current] : [],
      loading: false,
      save: async () => {},
      forCalendar: (calendarId: string) =>
        calendarId === boundSignature.calendarId ? boundSignature.current : null,
      bind: async () => {},
    }),
  };
});
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
  calendarRows.current = [];
  ownAddress.current = null;
  ownAddress.answer = null;
  boundSignature.current = null;
  boundSignature.calendarId = 'cal-local';
  CALENDARS.splice(1);
  STORE.selectedCalendarIds = new Set(['cal-local']);
  delete (CALENDARS[0] as { invitation_import?: string }).invitation_import;
  (CALENDARS[0] as { keeps_uid?: boolean }).keeps_uid = true;
});

/** Once the calendar was asked whether it holds the appointment already. */
async function asked() {
  await waitFor(() =>
    expect(invokeMock.mock.calls.some(([command]) => command === 'get_events')).toBe(true),
  );
  await act(async () => {});
}

function createRequest(): Record<string, unknown> {
  const create = invokeMock.mock.calls.find(([command]) => command === 'create_event');
  return (create?.[1] as { request: Record<string, unknown> }).request;
}

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

async function openImport(onImported = () => {}, importFrom: ImportedEvent = SERIES) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog
        isOpen
        onClose={() => {}}
        event={null}
        importFrom={importFrom}
        importNote="Nicht übernommen: die Aufgabe „Geschenke besorgen“."
        onImported={onImported}
      />
    </StrictMode>,
  );
  // What the editor loads beside the form (sounds, the signature, the
  // calendar's answers) lands inside act; a pending answer stays out.
  await act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
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
    await asked();
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
    // The file's UID, so importing it again finds it (I4).
    expect(request.uid_to_keep).toBe('teams-series-0001@example.org');
    await waitFor(() => expect(onImported).toHaveBeenCalledTimes(1));
  });

  it('says when the calendar cannot tell whether it holds the appointment', async () => {
    (CALENDARS[0] as { keeps_uid?: boolean }).keeps_uid = false;
    await openImport();
    expect(
      await screen.findByText(
        '„Kalender“ behält die Kennung einer Datei nicht: Aperio kann dort nicht erkennen, ob ' +
          'dieser Termin schon da ist.',
      ),
    ).toBeTruthy();
    expect(invokeMock.mock.calls.some(([command]) => command === 'get_events')).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() => expect(createRequest()).toBeTruthy());
  });

  it('gives a series the file keeps in UTC no zone of the device', async () => {
    const { EventDialog } = await import('./EventDialog');
    const utcSeries: ImportedEvent = {
      ...SERIES,
      recurrence: { rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] },
    };
    render(<EventDialog isOpen onClose={() => {}} event={null} importFrom={utcSeries} />);
    await asked();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    const create = invokeMock.mock.calls.find(([command]) => command === 'create_event');
    const request = (create?.[1] as { request: { recurrence: Record<string, unknown> } }).request;
    expect(request.recurrence).toEqual({ rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] });
  });

  it('creates a copy when the calendar holds it already, and says so first', async () => {
    calendarRows.current = [{ ical_uid: 'teams-series-0001@example.org' }];
    await openImport();
    expect(
      await screen.findByText(
        'Dieser Termin steht schon in „Kalender“. Anlegen legt eine Kopie an.',
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    expect(createRequest().uid_to_keep).toBeNull();
  });

  it('creates nothing while it still asks', async () => {
    calendarRows.current = 'pending';
    await openImport();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    expect(
      await screen.findByText(
        'Aperio prüft noch, ob dieser Termin schon in „Kalender“ steht. Versuche es gleich noch einmal.',
      ),
    ).toBeTruthy();
    expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(false);
  });
});

describe('EventDialog → an invitation from a file, as the calendar takes it (I6)', () => {
  const INVITATION: ImportedEvent = {
    ...SERIES,
    organizer: 'boss@example.org',
    attendees: ['Toni <toni@example.org>', 'gast@example.org'],
  };

  it('keeps the account’s signature out of someone else’s invitation', async () => {
    // Moved from one calendar that keeps invitations to another that carries
    // a signature: the form is not filled from the file again, so only the
    // lock keeps the block out.
    (CALENDARS[0] as { invitation_import?: string }).invitation_import = 'kept';
    CALENDARS.push({
      id: 'cal-office',
      name: 'Büro',
      read_only: false,
      account_id: 'local',
      invitation_import: 'kept',
      keeps_uid: true,
    } as unknown as Calendar);
    STORE.selectedCalendarIds = new Set(['cal-local', 'cal-office']);
    ownAddress.current = 'toni@example.org';
    boundSignature.current = { id: 'sig-1', name: 'Büro', body: 'Grüße, Toni' };
    boundSignature.calendarId = 'cal-office';
    await openImport(() => {}, INVITATION);
    await screen.findByText(/Eine Einladung von boss@example\.org/);
    fireEvent.change(screen.getByRole('combobox', { name: /^kalender$/i }), {
      target: { value: 'cal-office' },
    });
    await screen.findByText(/Eine Einladung von boss@example\.org/);
    await asked();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    expect(createRequest().description).toBe('Teams-Besprechung');
  });

  it('gives the account’s own meeting from a file no signature either', async () => {
    // One rule for every import (Toni, 2026-10-26): the description stays as
    // the file gave it, though the calendar carries a signature and the
    // fields are open.
    ownAddress.current = 'boss@example.org';
    boundSignature.current = { id: 'sig-1', name: 'Büro', body: 'Grüße, Toni' };
    boundSignature.calendarId = 'cal-local';
    await openImport(() => {}, INVITATION);
    await asked();
    screen.getByRole('combobox', { name: /^titel$/i });
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    expect(createRequest().description).toBe('Teams-Besprechung');
  });

  it('keeps focus on the title when the fields open after the owner check', async () => {
    // Nothing before the title: it is the first stop, locked while it is
    // asked whose the meeting is.
    ownAddress.current = 'later';
    const { EventDialog } = await import('./EventDialog');
    render(
      <EventDialog isOpen onClose={() => {}} event={null} importFrom={INVITATION} importNote={null} />,
    );
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
    const locked = screen.getByDisplayValue('Jour fixe');
    locked.focus();
    expect(document.activeElement).toBe(locked);
    // The account's own meeting: the title becomes a field to type into.
    await act(async () => {
      ownAddress.answer?.('boss@example.org');
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    const title = screen.getByRole('combobox', { name: /^titel$/i });
    expect(document.activeElement).toBe(title);
  });

  it('keeps someone else’s invitation where the calendar keeps invitations, locked as received', async () => {
    (CALENDARS[0] as { invitation_import?: string }).invitation_import = 'kept';
    ownAddress.current = 'toni@example.org';
    await openImport(() => {}, INVITATION);
    expect(
      await screen.findByText(
        'Eine Einladung von boss@example.org: Organisator und Teilnehmer bleiben, und niemand wird benachrichtigt.',
      ),
    ).toBeTruthy();
    // The content as the organizer sent it: no field to type into.
    expect(screen.queryByRole('combobox', { name: /^titel$/i })).toBeNull();
    expect(screen.queryByRole('checkbox', { name: /benachrichtigen/i })).toBeNull();
    await asked();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    expect(createRequest()).toMatchObject({
      title: 'Jour fixe',
      organizer: 'boss@example.org',
      organized_elsewhere: true,
      keeps_invitation: true,
      attendees: ['Toni <toni@example.org>', 'gast@example.org'],
      send_invitations: false,
    });
  });

  it('sets it aside where the calendar cannot keep it quietly, as the account’s own', async () => {
    ownAddress.current = 'toni@example.org';
    await openImport(() => {}, INVITATION);
    expect(
      await screen.findByText(/In „Kalender“ lassen sich Organisator und Teilnehmer nicht übernehmen/),
    ).toBeTruthy();
    // The account's own appointment: its content can be changed.
    expect(screen.getByRole('combobox', { name: /^titel$/i })).toBeTruthy();
    await asked();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    expect(createRequest()).toMatchObject({
      attendees: [],
      organizer: null,
      organized_elsewhere: false,
      keeps_invitation: false,
    });
  });

  it('takes the account’s own meeting as a new appointment, with its invitees', async () => {
    ownAddress.current = 'MAILTO:Boss@Example.org';
    await openImport(() => {}, INVITATION);
    expect(await screen.findByRole('combobox', { name: /^titel$/i })).toBeTruthy();
    expect(screen.queryByText(/Eine Einladung von/)).toBeNull();
    await asked();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(true),
    );
    expect(createRequest()).toMatchObject({
      attendees: ['Toni <toni@example.org>', 'gast@example.org'],
      organizer: 'boss@example.org',
      organized_elsewhere: false,
      keeps_invitation: false,
    });
  });

  it('creates nothing while it still asks whose it is', async () => {
    ownAddress.current = 'pending';
    await openImport(() => {}, INVITATION);
    await asked();
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    expect(
      await screen.findByText(
        'Aperio prüft noch, ob du diesen Termin selbst organisierst. Versuche es gleich noch einmal.',
      ),
    ).toBeTruthy();
    expect(invokeMock.mock.calls.some(([command]) => command === 'create_event')).toBe(false);
  });
});
