import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, FileImport, ImportedEvent } from '../api/types';
import i18n from '../i18n';

const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);

/**
 * The import dialog (DESIGN-file-import.md): a file of several appointments
 * as a list, each to open in the editor or all chosen ones written to one
 * calendar, one after another; a file with nothing to import, or one that
 * failed, as a sentence.
 */

const failTitles = vi.hoisted(() => ({ current: new Set<string>() }));
/** Creates that land and then fail at a later step: they stand in part. */
const inPartTitles = vi.hoisted(() => ({ current: new Set<string>() }));
/** Creates whose answer was lost: they may stand. */
const lostTitles = vi.hoisted(() => ({ current: new Set<string>() }));
/** Creates held open until the test lets them go: a list that is writing. */
const heldCreates = vi.hoisted(() => ({ on: false, release: [] as Array<() => void> }));
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
      if (inPartTitles.current.has(request.title)) {
        return Promise.reject({ code: 'network', message: 'partly-saved: reply' });
      }
      if (lostTitles.current.has(request.title)) {
        return Promise.reject({ code: 'network', message: 'connection reset' });
      }
      if (heldCreates.on) {
        return new Promise((resolve) =>
          heldCreates.release.push(() => resolve({ ...request, id: `id-${request.title}` })),
        );
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
  { id: 'cal-home', name: 'Privat', read_only: false, account_id: 'local', keeps_uid: true },
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'local', keeps_uid: true },
] as unknown as Calendar[];
const STORE = {
  calendars: CALENDARS,
  selectedCalendarIds: new Set(['cal-home', 'cal-work']) as ReadonlySet<string>,
};
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
  inPartTitles.current = new Set();
  lostTitles.current = new Set();
  heldCreates.on = false;
  heldCreates.release = [];
  STORE.selectedCalendarIds = new Set(['cal-home', 'cal-work']);
  calendarRows.current = {};
  ownAddress.current = {};
  for (const calendar of CALENDARS) {
    delete (calendar as { invitation_import?: string }).invitation_import;
    delete (calendar as { recurrence_capabilities?: unknown }).recurrence_capabilities;
    (calendar as { keeps_uid?: boolean }).keeps_uid = true;
  }
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
  // A read of its own, as every file read is: the list keeps its progress by
  // the read, and the cases here share their files.
  const fresh = 'read' in outcome ? { read: structuredClone(outcome.read) } : outcome;
  await openSame(fresh, onClose);
  return onClose;
}

/** Open the list on exactly this outcome, as the dialog stack does when it
 *  shows a frame again. */
async function openSame(outcome: { read: FileImport } | { failed: unknown }, onClose = vi.fn()) {
  const { ImportDialog } = await import('./ImportDialog');
  render(<ImportDialog isOpen onClose={onClose} fileName="export.ics" outcome={outcome} />);
  // The calendar's answers (the account's address, what it holds) land inside
  // act, as a person would see them after a moment; a pending one stays out.
  await act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
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
      // Each one that did not arrive with its own reason, a busy provider in
      // its own words and asked to wait (decisions 270, 272); nothing after
      // that would say nothing was created.
      await screen.findByText(
        '1 von 2 Terminen in „Privat“ importiert. Nicht importiert: „Projektbesprechung“ ' +
          '(der Anbieter ist gerade ausgelastet). Versuche es später noch einmal.',
      ),
    ).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    // The one that arrived is marked and no longer offered; the button now
    // offers the one left.
    expect(screen.getAllByRole('checkbox')).toHaveLength(1);
    expect(screen.getByText(/Chorprobe, .*, importiert/)).toBeTruthy();
    expect(screen.getByRole('button', { name: '1 Termin importieren' })).toBeTruthy();
  });

  it('marks one that stands in part and does not offer it again', async () => {
    // Review round 15 of #128: it read as not imported and stayed checked,
    // so the next press made it twice.
    inPartTitles.current = new Set(['Projektbesprechung']);
    const onClose = await open({ read: READ });
    await screen.findByText(/In „Privat“ lassen sich Organisator und Teilnehmer/);
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    expect(
      await screen.findByText(
        /^1 von 2 Terminen ganz in „Privat“ importiert\. Vielleicht nicht ganz angekommen, prüfe ihn im Kalender: „Projektbesprechung“/,
      ),
    ).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryAllByRole('checkbox')).toHaveLength(0);
    expect(screen.getByText(/Projektbesprechung, .*, vielleicht nicht ganz importiert/)).toBeTruthy();
  });

  it('does not offer again one whose answer was lost', async () => {
    // Review round 16 of #128: a create that may have landed read as not
    // imported and stayed checked.
    lostTitles.current = new Set(['Projektbesprechung']);
    await open({ read: READ });
    await screen.findByText(/In „Privat“ lassen sich Organisator und Teilnehmer/);
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    expect(await screen.findByText(/Vielleicht nicht ganz angekommen/)).toBeTruthy();
    expect(screen.queryAllByRole('checkbox')).toHaveLength(0);
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

  it('keeps what it did while the editor sits on top of it', async () => {
    // The dialog stack shows only its top frame: "Edit …" unmounts the list,
    // and it comes back on the same read when the editor closes.
    const outcome = { read: structuredClone(READ) };
    await openSame(outcome);
    fireEvent.click(screen.getByRole('checkbox', { name: /^Chorprobe, / }));
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    fireEvent.click(screen.getByRole('button', { name: '„Projektbesprechung“ im Editor öffnen' }));
    const [, options] = openEventDialog.mock.calls[0];
    cleanup();
    act(() => options.onImported());

    await openSame(outcome);
    expect(screen.getByText(/Projektbesprechung, .*, importiert/)).toBeTruthy();
    expect((screen.getByRole('checkbox', { name: /^Chorprobe, / }) as HTMLInputElement).checked).toBe(
      false,
    );
    expect((screen.getByLabelText('In Kalender') as HTMLSelectElement).value).toBe('cal-work');
    // Nothing left to import: the one created is not written again, the one
    // unchecked stays out.
    fireEvent.click(screen.getByRole('button', { name: '0 Termine importieren' }));
    expect(await screen.findByText('Kein Termin ausgewählt.')).toBeTruthy();
    expect(createRequests()).toEqual([]);
  });

  it('marks an arrived row as a stop of its own', async () => {
    await open({ read: READ });
    fireEvent.click(screen.getByRole('button', { name: '„Projektbesprechung“ im Editor öffnen' }));
    const [, options] = openEventDialog.mock.calls[0];
    act(() => options.onImported());
    const done = screen.getByText(/Projektbesprechung, .*, importiert/);
    expect(done.getAttribute('tabindex')).toBe('0');
  });

  it('holds the "please wait" dialog over every series it writes in steps', async () => {
    // Review round 16 of #128: it opened and closed again for each series,
    // and focus went back to the button between them.
    (CALENDARS[0] as { recurrence_capabilities?: unknown }).recurrence_capabilities = {
      deletions_in_steps: true,
    };
    heldCreates.on = true;
    const series = (title: string) =>
      event(title, {
        recurrence: {
          rrule: 'FREQ=WEEKLY',
          exceptions: ['2026-11-25T13:00:00Z'],
        } as ImportedEvent['recurrence'],
      });
    await open({ read: { events: [series('Jour fixe'), series('Chor')], skipped: [] } });
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    const waiting = () => screen.queryByRole('dialog', { name: /Vorgang läuft/ });
    await waitFor(() => expect(waiting()).toBeTruthy());
    const opened = waiting();
    await act(async () => {
      heldCreates.release.shift()?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    // The same dialog, never closed between the two.
    expect(waiting()).toBe(opened);
    await act(async () => {
      heldCreates.release.shift()?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    await waitFor(() => expect(waiting()).toBeNull());
  });

  it('writes someone else’s series at once where the calendar keeps it as theirs', async () => {
    // Review round 21 of #128: Exchange and Google take such an invitation in
    // one request, its deletions with it; nothing is written in steps.
    (CALENDARS[0] as { recurrence_capabilities?: unknown }).recurrence_capabilities = {
      deletions_in_steps: true,
    };
    (CALENDARS[0] as { invitation_import?: string }).invitation_import = 'kept';
    heldCreates.on = true;
    const invitation = event('Jour fixe', {
      organizer: 'erika@example.org',
      attendees: ['Max <max@example.org>'],
      recurrence: {
        rrule: 'FREQ=WEEKLY',
        exceptions: ['2026-11-25T13:00:00Z'],
      } as ImportedEvent['recurrence'],
    });
    await open({ read: { events: [invitation], skipped: [] } });
    const button = await screen.findByRole('button', { name: '1 Termin importieren' });
    await waitFor(() => expect(button.getAttribute('aria-disabled')).not.toBe('true'));
    fireEvent.click(button);
    await waitFor(() => expect(createRequests()).toHaveLength(1));
    expect(createRequests()[0].keeps_invitation).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByRole('dialog', { name: /Vorgang läuft/ })).toBeNull();
    await act(async () => {
      heldCreates.release.shift()?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  });

  it('holds the calendar and the rows while it writes', async () => {
    heldCreates.on = true;
    await open({ read: READ });
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(1));
    const picker = screen.getByLabelText('In Kalender') as HTMLSelectElement;
    expect(picker.getAttribute('aria-disabled')).toBe('true');
    fireEvent.change(picker, { target: { value: 'cal-work' } });
    expect(picker.value).toBe('cal-home');
    const row = screen.getByRole('checkbox', { name: /^Projektbesprechung, / }) as HTMLInputElement;
    expect(row.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(row);
    expect(row.checked).toBe(true);
    await act(async () => {
      while (heldCreates.release.length > 0) heldCreates.release.shift()?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    await waitFor(() => expect(createRequests()).toHaveLength(2));
    await act(async () => {
      while (heldCreates.release.length > 0) heldCreates.release.shift()?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  });

  it('says there is no calendar to write to, and writes nothing', async () => {
    STORE.selectedCalendarIds = new Set();
    await open({ read: READ });
    expect(screen.queryByText(/„“/)).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    expect(
      await screen.findByText('Es gibt keinen Kalender, in den Aperio Termine schreiben kann.'),
    ).toBeTruthy();
    expect(createRequests()).toEqual([]);
  });

  it('focuses and says the same notice again for a second press', async () => {
    await open({ read: READ });
    for (const row of screen.getAllByRole('checkbox')) fireEvent.click(row);
    const button = screen.getByRole('button', { name: '0 Termine importieren' });
    fireEvent.click(button);
    const first = await screen.findByText('Kein Termin ausgewählt.');
    expect(document.activeElement).toBe(first);
    button.focus();
    fireEvent.click(button);
    const second = await screen.findByText('Kein Termin ausgewählt.');
    await waitFor(() => expect(document.activeElement).toBe(second));
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

  it('keeps a held row checked as a copy when the list comes back from the editor', async () => {
    calendarRows.current = { 'cal-home': [{ ical_uid: 'chor-1@example.org' }] };
    const outcome = { read: structuredClone(HELD_READ) };
    await openSame(outcome);
    const held = await screen.findByRole('checkbox', {
      name: /^Chorprobe, .*, steht schon in „Privat“$/,
    });
    fireEvent.click(held);
    fireEvent.click(screen.getByRole('button', { name: '„Kassenprüfung“ im Editor öffnen' }));
    cleanup();
    // The same answer comes again on the way back: taken once, it does not
    // uncheck what the person checked since.
    await openSame(outcome);
    const again = await screen.findByRole('checkbox', {
      name: /^Chorprobe, .*, steht schon in „Privat“, wird als Kopie importiert$/,
    });
    expect((again as HTMLInputElement).checked).toBe(true);
  });

  it('asks nothing of a calendar that keeps no file’s identity, and says so', async () => {
    (CALENDARS[0] as { keeps_uid?: boolean }).keeps_uid = false;
    await open({ read: HELD_READ });
    expect(
      await screen.findByText(
        '„Privat“ behält die Kennung einer Datei nicht: Aperio kann dort nicht erkennen, was ' +
          'davon schon da ist.',
      ),
    ).toBeTruthy();
    expect(invokeMock.mock.calls.some(([command]) => command === 'get_events')).toBe(false);
    // Nothing waits for an answer that never comes: the import goes on.
    fireEvent.click(screen.getByRole('button', { name: '2 Termine importieren' }));
    await waitFor(() => expect(createRequests()).toHaveLength(2));
  });

  it('checks rows again and says so when the calendar chosen cannot tell', async () => {
    calendarRows.current = { 'cal-home': [{ ical_uid: 'chor-1@example.org' }] };
    (CALENDARS[1] as { keeps_uid?: boolean }).keeps_uid = false;
    await open({ read: HELD_READ });
    const chor = await screen.findByRole('checkbox', {
      name: /^Chorprobe, .*, steht schon in „Privat“$/,
    });
    expect((chor as HTMLInputElement).checked).toBe(false);
    announce.mockClear();
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    const again = await screen.findByRole('checkbox', { name: /^Chorprobe, [^,]*, [^,]*$/ });
    expect((again as HTMLInputElement).checked).toBe(true);
    expect(announce).toHaveBeenCalledWith(
      '„Arbeit“ behält die Kennung einer Datei nicht: Aperio kann dort nicht erkennen, was ' +
        'davon schon da ist.',
    );
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

  it('says how a calendar takes the invitations when another is chosen', async () => {
    (CALENDARS[1] as { invitation_import?: string }).invitation_import = 'kept';
    await open({ read: READ });
    const setAside = t('fileImport.invitation.listSetAside', { calendar: 'Privat' });
    await waitFor(() => expect(announce).toHaveBeenCalledWith(setAside));
    announce.mockClear();
    fireEvent.change(screen.getByLabelText('In Kalender'), { target: { value: 'cal-work' } });
    await waitFor(() =>
      expect(announce).toHaveBeenCalledWith(
        t('fileImport.invitation.listKept', { calendar: 'Arbeit' }),
      ),
    );
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
