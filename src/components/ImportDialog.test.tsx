import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, FileImport, ImportedEvent } from '../api/types';

/**
 * The import dialog (DESIGN-file-import.md): a file of several appointments
 * as a list, each to open in the editor or all chosen ones written to one
 * calendar, one after another; a file with nothing to import, or one that
 * failed, as a sentence.
 */

const failTitles = vi.hoisted(() => ({ current: new Set<string>() }));
/** What each calendar holds when asked (`get_events`): rows, a failure, or an
 *  answer that does not come. */
const calendarRows = vi.hoisted(() => ({
  current: {} as Record<string, Array<{ ical_uid?: string }> | 'fail' | 'pending'>,
}));
/** The account's address in each calendar (`calendar_current_user_email`). */
const ownAddress = vi.hoisted(() => ({
  current: {} as Record<string, string | null | 'pending'>,
}));
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
    if (command === 'get_events') {
      const request = (payload as { request: { calendar_id: string } }).request;
      const rows = calendarRows.current[request.calendar_id] ?? [];
      if (rows === 'fail') return Promise.reject({ code: 'network', message: 'offline' });
      if (rows === 'pending') return new Promise(() => {});
      return Promise.resolve(rows);
    }
    if (command === 'create_event') {
      const request = (payload as { request: { title: string } }).request;
      if (failTitles.current.has(request.title)) {
        return Promise.reject({ code: 'forbidden', message: 'provider-busy: http-429' });
      }
      return Promise.resolve({ ...request, id: `id-${request.title}` });
    }
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS = [
  { id: 'cal-home', name: 'Privat', read_only: false, account_id: 'local' },
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'local' },
] as unknown as Calendar[];
const STORE = { calendars: CALENDARS, selectedCalendarIds: new Set(['cal-home', 'cal-work']) };
const VIEW_STATE = { showHiddenCalendarTargets: false };
const openEventDialog = vi.fn();
const invalidateData = vi.fn();
const announce = vi.fn();
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ openEventDialog, invalidateData }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
// Its own answers per test: the real one keeps them for the page's life.
vi.mock('../state/currentUserEmail', () => ({
  resolveCalendarUserEmail: (calendarId: string) =>
    ownAddress.current[calendarId] === 'pending'
      ? new Promise(() => {})
      : Promise.resolve(ownAddress.current[calendarId] ?? null),
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  openEventDialog.mockClear();
  invalidateData.mockClear();
  announce.mockClear();
  failTitles.current = new Set();
  calendarRows.current = {};
  ownAddress.current = {};
  for (const calendar of CALENDARS) delete (calendar as { invitation_import?: string }).invitation_import;
  localStorage.clear();
});

const event = (title: string, extra: Partial<ImportedEvent> = {}): ImportedEvent => ({
  title,
  start: '2026-11-18T13:00:00Z',
  end: '2026-11-18T14:00:00Z',
  all_day: false,
  reminders: [],
  attendees: [],
  ...extra,
});

const READ: FileImport = {
  events: [
    event('Chorprobe'),
    event('Projektbesprechung', {
      organizer: 'erika@example.org',
      attendees: ['Max <max@example.org>'],
    }),
  ],
  skipped: [{ kind: 'task', title: 'Geschenke besorgen' }],
};

async function open(outcome: { read: FileImport } | { failed: unknown }, onClose = vi.fn()) {
  const { ImportDialog } = await import('./ImportDialog');
  render(<ImportDialog isOpen onClose={onClose} fileName="export.ics" outcome={outcome} />);
  return onClose;
}

function createRequests(): Array<Record<string, unknown>> {
  return invokeMock.mock.calls
    .filter(([command]) => command === 'create_event')
    .map(([, payload]) => (payload as { request: Record<string, unknown> }).request);
}

describe('ImportDialog → several appointments', () => {
  it('says what the file holds and what it leaves out, first', async () => {
    await open({ read: READ });
    expect(
      await screen.findByText(
        'In „export.ics“ stehen 2 Termine. Nicht übernommen: die Aufgabe „Geschenke besorgen“. ' +
          'In „Privat“ lassen sich Organisator und Teilnehmer von Einladungen nicht übernehmen, ' +
          'ohne dass jemand benachrichtigt wird; solche Termine werden deine eigenen, ohne Teilnehmer.',
      ),
    ).toBeTruthy();
    expect(screen.getAllByRole('checkbox')).toHaveLength(2);
  });

  it('writes the chosen ones to the chosen calendar, an invitation without its people', async () => {
    const onClose = await open({ read: READ });
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    // Whose the invitation is decides what it keeps.
    await screen.findByText(/In „Arbeit“ lassen sich Organisator und Teilnehmer/);
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    const requests = createRequests();
    expect(requests.map((r) => r.title)).toEqual(['Chorprobe', 'Projektbesprechung']);
    expect(requests.every((r) => r.calendar_id === 'cal-work')).toBe(true);
    expect(requests[1].attendees).toEqual([]);
    expect(requests[1].organizer).toBeNull();
    expect(requests[1].keeps_invitation).toBe(false);
    expect(requests.every((r) => r.send_invitations === false)).toBe(true);
    expect(announce).toHaveBeenCalledWith('2 Termine in „Arbeit“ importiert.');
    expect(invalidateData).toHaveBeenCalled();
  });

  it("keeps each appointment's UID from the file, so importing it again finds it", async () => {
    await open({
      read: {
        events: [event('Chorprobe', { uid: 'chor-1@example.org' }), event('Ohne Kennung')],
        skipped: [],
      },
    });
    // Which ones are there already decides which keep their UID.
    expect(await screen.findByText('Nichts davon steht schon in „Privat“.')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    expect(createRequests().map((r) => r.uid_to_keep)).toEqual(['chor-1@example.org', null]);
  });

  it('leaves out what was unchecked', async () => {
    await open({ read: READ });
    fireEvent.click(screen.getByRole('checkbox', { name: /Chorprobe/ }));
    fireEvent.click(screen.getByRole('button', { name: '1 Termin importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(1));
    expect(createRequests()[0].title).toBe('Projektbesprechung');
  });

  it('says which did not arrive and keeps the rest marked', async () => {
    failTitles.current = new Set(['Projektbesprechung']);
    const onClose = await open({ read: READ });
    await screen.findByText(/In „Privat“ lassen sich Organisator und Teilnehmer/);
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    expect(
      await screen.findByText(
        /^1 von 2 Terminen in „Privat“ importiert\. Nicht importiert: „Projektbesprechung“\./,
      ),
    ).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    // The one that arrived is marked and no longer offered; the button now
    // offers the one left.
    expect(screen.getAllByRole('checkbox')).toHaveLength(1);
    expect(screen.getByText(/Chorprobe, .*, importiert/)).toBeTruthy();
    expect(screen.getByRole('button', { name: '1 Termin importieren' })).toBeTruthy();
  });

  it('opens one in the editor, as it stands, and marks it once created', async () => {
    await open({ read: READ });
    fireEvent.click(screen.getByRole('button', { name: '„Projektbesprechung“ im Editor öffnen' }));
    expect(openEventDialog).toHaveBeenCalledTimes(1);
    const [, options] = openEventDialog.mock.calls[0];
    expect(options.calendarId).toBe('cal-home');
    // As the file has it: the editor decides what the calendar keeps (I6).
    expect(options.importFrom.title).toBe('Projektbesprechung');
    expect(options.importFrom.attendees).toEqual(['Max <max@example.org>']);
    expect(options.importFrom.organizer).toBe('erika@example.org');
    expect(options.importNote).toBeNull();
    act(() => options.onImported());
    expect(screen.getByText(/Projektbesprechung, .*, importiert/)).toBeTruthy();
  });
});

describe('ImportDialog → what the calendar holds already (I4)', () => {
  const HELD_READ: FileImport = {
    events: [
      event('Chorprobe', { uid: 'chor-1@example.org' }),
      event('Kassenprüfung', { uid: 'kasse-1@example.org' }),
    ],
    skipped: [],
  };

  it('unchecks and names what is there, and imports it as a copy when checked', async () => {
    calendarRows.current = { 'cal-home': [{ ical_uid: 'chor-1@example.org' }] };
    await open({ read: HELD_READ });
    expect(screen.getByText('Aperio prüft, was davon schon in „Privat“ steht …')).toBeTruthy();
    const held = await screen.findByRole('checkbox', {
      name: /^Chorprobe, .*, steht schon in „Privat“$/,
    });
    expect((held as HTMLInputElement).checked).toBe(false);
    const sentence =
      'Schon in „Privat“: „Chorprobe“. Abgewählt; wer einen anwählt, importiert ihn als Kopie.';
    expect(screen.getByText(sentence)).toBeTruthy();
    expect(announce).toHaveBeenCalledWith(sentence);

    fireEvent.click(held);
    expect(
      screen.getByRole('checkbox', {
        name: /^Chorprobe, .*, steht schon in „Privat“, wird als Kopie importiert$/,
      }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    expect(createRequests().map((r) => [r.title, r.uid_to_keep])).toEqual([
      ['Chorprobe', null],
      ['Kassenprüfung', 'kasse-1@example.org'],
    ]);
  });

  it('checks a row again in a calendar that does not hold it', async () => {
    calendarRows.current = {
      'cal-home': [{ ical_uid: 'chor-1@example.org' }],
      'cal-work': [{ ical_uid: 'kasse-1@example.org' }],
    };
    await open({ read: HELD_READ });
    await screen.findByRole('checkbox', { name: /^Chorprobe, .*, steht schon in „Privat“$/ });
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    const kasse = await screen.findByRole('checkbox', {
      name: /^Kassenprüfung, .*, steht schon in „Arbeit“$/,
    });
    expect((kasse as HTMLInputElement).checked).toBe(false);
    const chor = screen.getByRole('checkbox', { name: /^Chorprobe, [^,]*, [^,]*$/ });
    expect((chor as HTMLInputElement).checked).toBe(true);
  });

  it('never lets an answer about one calendar stand for another', async () => {
    calendarRows.current = {
      'cal-home': [{ ical_uid: 'chor-1@example.org' }],
      'cal-work': 'pending',
    };
    await open({ read: HELD_READ });
    await screen.findByRole('checkbox', { name: /^Chorprobe, .*, steht schon in „Privat“$/ });
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    expect(await screen.findByText('Aperio prüft, was davon schon in „Arbeit“ steht …')).toBeTruthy();
    expect(screen.queryByText(/steht schon in „Arbeit“/)).toBeNull();
  });

  it('says when it could not ask, and keeps every UID', async () => {
    calendarRows.current = { 'cal-home': 'fail' };
    await open({ read: HELD_READ });
    const failed = 'Aperio konnte nicht prüfen, was davon schon in „Privat“ steht.';
    expect(await screen.findByText(failed)).toBeTruthy();
    expect(announce).toHaveBeenCalledWith(failed);
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    expect(createRequests().map((r) => r.uid_to_keep)).toEqual([
      'chor-1@example.org',
      'kasse-1@example.org',
    ]);
  });

  it('imports nothing while it still asks', async () => {
    calendarRows.current = { 'cal-home': 'pending' };
    await open({ read: HELD_READ });
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    expect(
      await screen.findByText(
        'Aperio prüft noch, was davon schon in „Privat“ steht. Versuche es gleich noch einmal.',
      ),
    ).toBeTruthy();
    expect(createRequests()).toHaveLength(0);
  });

  it('asks nothing and says nothing for a file without UIDs', async () => {
    await open({ read: READ });
    expect(invokeMock.mock.calls.some(([command]) => command === 'get_events')).toBe(false);
    expect(screen.queryByText(/schon in „Privat“/)).toBeNull();
  });
});

describe('ImportDialog → invitations, as the calendar takes them (I6)', () => {
  it('keeps an invitation as someone else’s where the calendar keeps invitations', async () => {
    (CALENDARS[0] as { invitation_import?: string }).invitation_import = 'kept-tentative';
    await open({ read: READ });
    expect(
      await screen.findByText(
        /Einladungen von anderen behalten in „Privat“ Organisator und Teilnehmer, und niemand wird benachrichtigt\. Deine Antwort steht danach auf „Mit Vorbehalt“\./,
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    const [plain, invitation] = createRequests();
    expect(plain.keeps_invitation).toBe(false);
    expect(invitation).toMatchObject({
      attendees: ['Max <max@example.org>'],
      organizer: 'erika@example.org',
      organized_elsewhere: true,
      keeps_invitation: true,
      send_invitations: false,
    });
  });

  it('imports nothing while it still asks whose they are', async () => {
    ownAddress.current = { 'cal-home': 'pending' };
    await open({ read: READ });
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    expect(
      await screen.findByText(
        'Aperio prüft noch, welche dieser Termine du selbst organisierst. Versuche es gleich noch einmal.',
      ),
    ).toBeTruthy();
    expect(createRequests()).toHaveLength(0);
  });

  it('takes the account’s own meeting as a new appointment, with its invitees', async () => {
    ownAddress.current = { 'cal-home': 'Erika@Example.org' };
    await open({ read: READ });
    await waitFor(() =>
      expect(screen.queryByText(/Organisator und Teilnehmer/)).toBeNull(),
    );
    // Nothing is said about invitations: there is none of someone else's.
    expect(
      await screen.findByText(
        'In „export.ics“ stehen 2 Termine. Nicht übernommen: die Aufgabe „Geschenke besorgen“.',
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    expect(createRequests()[1]).toMatchObject({
      attendees: ['Max <max@example.org>'],
      organized_elsewhere: false,
      keeps_invitation: false,
    });
  });
});

describe('ImportDialog → nothing to import', () => {
  it('says the file holds no appointment, and what it held', async () => {
    await open({ read: { events: [], skipped: [{ kind: 'cancelled', title: 'Abgesagt: X' }] } });
    expect(
      screen.getByText(
        'In „export.ics“ steht kein Termin, der sich importieren lässt. ' +
          'Nicht übernommen: die Absage „Abgesagt: X“.',
      ),
    ).toBeTruthy();
  });

  it('says a failure in words', async () => {
    await open({ failed: { code: 'unsupported', message: 'no import format reads export.ics' } });
    expect(
      screen.getByText(
        '„export.ics“ ließ sich nicht importieren: Aperio kann Dateien dieser Art nicht importieren.',
      ),
    ).toBeTruthy();
  });
});
