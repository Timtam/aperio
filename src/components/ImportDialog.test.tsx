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
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
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

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  openEventDialog.mockClear();
  invalidateData.mockClear();
  announce.mockClear();
  failTitles.current = new Set();
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
      screen.getByText(
        'In „export.ics“ stehen 2 Termine. Nicht übernommen: die Aufgabe „Geschenke besorgen“. ' +
          'Teilnehmer und Organisator von Einladungen übernimmt Aperio noch nicht; solche Termine ' +
          'werden deine eigenen, ohne Teilnehmer.',
      ),
    ).toBeTruthy();
    expect(screen.getAllByRole('checkbox')).toHaveLength(2);
  });

  it('writes the chosen ones to the chosen calendar, an invitation without its people', async () => {
    const onClose = await open({ read: READ });
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    const requests = createRequests();
    expect(requests.map((r) => r.title)).toEqual(['Chorprobe', 'Projektbesprechung']);
    expect(requests.every((r) => r.calendar_id === 'cal-work')).toBe(true);
    expect(requests[1].attendees).toEqual([]);
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
    expect(options.importFrom.title).toBe('Projektbesprechung');
    expect(options.importFrom.attendees).toEqual([]);
    expect(options.importFrom.organizer).toBeUndefined();
    expect(options.importNote).toMatch(/noch nicht/);
    act(() => options.onImported());
    expect(screen.getByText(/Projektbesprechung, .*, importiert/)).toBeTruthy();
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
