import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { DEFAULT_RECURRENCE_CAPABILITIES, expandEvent, occurrenceOfSeries } from '@aperio/shared';

import type { Calendar, CalendarEvent, ImportedEvent } from '../api/types';

/**
 * The end of a repeat in the desktop editor (DESIGN-series-time-zone.md,
 * stage 12, part 3, 140a-142a): an end is a day on the series' clock, and only
 * the core says which day and how to spell it.
 *
 * A new series keeps the day typed in its end field while the form is open,
 * whatever its zone and time become, and is spelled through that day on the
 * facts it is saved with (141a); so does a file's, whose end is read once as a
 * day on the file's facts. A stored series' field reads and writes on the
 * series' zone and names it, also where the other fields stand on the
 * device's clock or on UTC; an end nobody changed goes back byte for byte.
 * With the real repeat controls, which the other editor tests replace.
 *
 * The device is Berlin here, whatever clock the machine runs on.
 */

const { invokeMock, announce, seriesById } = vi.hoisted(() => {
  const announce = () => {};
  /** The series `get_event_by_id` answers, by id. */
  const seriesById = new Map<string, unknown>();
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'get_event_by_id') {
      return Promise.resolve(seriesById.get((payload as { id: string }).id) ?? null);
    }
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'create_event') {
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'created-1' });
    }
    if (command === 'get_series_rows') {
      return Promise.resolve({ rows: [], reach: { kind: 'complete' } });
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, announce, seriesById };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

/** A calendar that stores a series' end as a date, as Exchange does, and one
 *  that does not, as CalDAV. */
const CALENDARS: Calendar[] = [
  {
    id: 'cal-work',
    name: 'Arbeit',
    read_only: false,
    account_id: 'acc-ews',
    recurrence_capabilities: { ...DEFAULT_RECURRENCE_CAPABILITIES, ends_by_date: true },
  } as unknown as Calendar,
  {
    id: 'cal-dav',
    name: 'Privat',
    read_only: false,
    account_id: 'acc-dav',
    recurrence_capabilities: { ...DEFAULT_RECURRENCE_CAPABILITIES },
  } as unknown as Calendar,
];

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work', 'cal-dav']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {} };
const REMINDERS = { getDefaultsFor: () => [] };

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
  seriesById.clear();
  vi.restoreAllMocks();
});

/** Pretend the device is in `zone`, whatever zone the test machine is in. */
function deviceIn(zone: string) {
  const real = new Intl.DateTimeFormat().resolvedOptions();
  vi.spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions').mockReturnValue({
    ...real,
    timeZone: zone,
  });
}

/** Pretend the device is in Berlin, whatever zone the test machine is in. */
function deviceInBerlin() {
  deviceIn('Europe/Berlin');
}

const calls = (command: string) => invokeMock.mock.calls.filter((call) => call[0] === command);

async function open(event: CalendarEvent | null, importFrom?: ImportedEvent) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} importFrom={importFrom} />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

const input = (label: RegExp) => screen.getByLabelText(label) as HTMLInputElement;
const zoneButton = () => screen.getByRole('button', { name: /^zeitzone der serie: /i });
const settle = () => act(async () => {});

/** Opens the picker from the zone button and picks the first row whose text
 *  starts with `row`, after searching for `query`. */
async function pick(row: string, query: string) {
  zoneButton().focus();
  fireEvent.click(zoneButton());
  const picker = await screen.findByRole('dialog', { name: /^zeitzone der serie$/i });
  fireEvent.change(within(picker).getByRole('combobox', { name: /^zeitzone suchen$/i }), {
    target: { value: query },
  });
  const option = within(picker)
    .getAllByRole('option')
    .find((candidate) => candidate.textContent?.startsWith(row));
  if (!option) throw new Error(`no row ${row}`);
  await act(async () => {
    fireEvent.mouseDown(option);
  });
  await settle();
}

/** The end field, by the clock its label names. */
const endField = (clock: string) => input(new RegExp(`^ende-datum \\(${clock}\\)$`, 'i'));

/** A new daily series on New York's clock, at `time` on 3 November, with its
 *  end typed as 10 November. */
async function newDailyInNewYork(time: string) {
  deviceInBerlin();
  await open(null);
  fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
    target: { value: 'Abendrunde' },
  });
  fireEvent.change(screen.getByRole('combobox', { name: /^wiederholung$/i }), {
    target: { value: 'DAILY' },
  });
  await pick('New York', 'New York');
  fireEvent.change(input(/^startdatum \(new york\)$/i), { target: { value: '2026-11-03' } });
  fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: time } });
  fireEvent.change(screen.getByRole('combobox', { name: /^ende$/i }), {
    target: { value: 'UNTIL' },
  });
  fireEvent.change(endField('new york'), { target: { value: '2026-11-10' } });
  expect(endField('new york').value).toBe('2026-11-10');
}

/** Saves the new series and answers the rule it was created with. */
async function created(): Promise<string> {
  fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
  await waitFor(() => expect(calls('create_event')).toHaveLength(1));
  return (calls('create_event')[0][1] as { request: { recurrence: { rrule: string } } }).request
    .recurrence.rrule;
}

describe('EventDialog → a new series keeps its typed end day (141a)', () => {
  it("spells the day through on the series' clock when it is saved", async () => {
    await newDailyInNewYork('19:00');
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
  });

  it('keeps the day while the time moves to the blind spot', async () => {
    await newDailyInNewYork('09:00');
    fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: '19:00' } });
    expect(endField('new york').value).toBe('2026-11-10');
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
  });

  it('keeps the day when another zone is chosen, and spells it on that clock', async () => {
    await newDailyInNewYork('19:00');
    await pick('Honolulu', 'Honolulu');
    expect(endField('honolulu').value).toBe('2026-11-10');
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261110T235959Z');
  });

  it('keeps the day east of UTC too', async () => {
    await newDailyInNewYork('19:00');
    await pick('Berlin', 'Berlin');
    // Berlin is the device's own clock: the label names none.
    fireEvent.change(input(/^startzeit$/i), { target: { value: '05:00' } });
    expect(input(/^ende-datum$/i).value).toBe('2026-11-10');
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261110T235959Z');
  });
});

describe("EventDialog → a stored series' end on its clock", () => {
  const STORED = {
    id: 'ev-ny',
    calendar_id: 'cal-work',
    title: 'Abendrunde',
    description: null,
    location: null,
    start: '2026-11-03T00:00:00.000Z',
    end: '2026-11-03T01:00:00.000Z',
    all_day: false,
    recurrence: { rrule: 'FREQ=DAILY;UNTIL=20261111T045959Z', exceptions: [], tzid: 'America/New_York' },
    color_label: null,
    reminders: [],
    attendees: [],
  } as unknown as CalendarEvent;

  it('names the clock, shows the day there and saves an untouched end byte for byte', async () => {
    deviceInBerlin();
    await open(STORED);
    expect(endField('new york').value).toBe('2026-11-10');
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
      target: { value: 'Abendrunde neu' },
    });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$/i }));
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = (calls('update_event')[0][1] as { event: CalendarEvent }).event;
    expect(sent.recurrence?.rrule).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
  });
});

describe('EventDialog → a single event that becomes a series keeps its typed end day (141a)', () => {
  /** A stored single event at `start`, an hour long, in `calendarId`. */
  const single = (start: string, calendarId: string) =>
    ({
      id: 'ev-single',
      calendar_id: calendarId,
      title: 'Abendrunde',
      description: null,
      location: null,
      start,
      end: new Date(Date.parse(start) + 3_600_000).toISOString(),
      all_day: false,
      recurrence: null,
      color_label: null,
      reminders: [],
      attendees: [],
    }) as unknown as CalendarEvent;

  /** Makes the open single event repeat daily through 10 November, in the
   *  end field that names `clock` (none for the device's own). */
  const dailyThroughTenth = (clock: string | null) => {
    fireEvent.change(screen.getByRole('combobox', { name: /^wiederholung$/i }), {
      target: { value: 'DAILY' },
    });
    fireEvent.change(screen.getByRole('combobox', { name: /^ende$/i }), {
      target: { value: 'UNTIL' },
    });
    const field = clock === null ? input(/^ende-datum$/i) : endField(clock);
    fireEvent.change(field, { target: { value: '2026-11-10' } });
  };

  async function saved(): Promise<string | undefined> {
    fireEvent.click(screen.getByRole('button', { name: /^speichern$/i }));
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    return (calls('update_event')[0][1] as { event: CalendarEvent }).event.recurrence?.rrule;
  }

  it('keeps the day on the clock chosen and saves through it there, on CalDAV', async () => {
    // 3 November, 19:00 New York, on a calendar that stores no end as a date.
    deviceIn('America/New_York');
    await open(single('2026-11-04T00:00:00.000Z', 'cal-dav'));
    dailyThroughTenth(null);
    await pick('Chicago', 'Chicago');
    expect(endField('chicago').value).toBe('2026-11-10');
    // Through 10 November 18:00 Chicago (11 November 00:00 UTC).
    expect(await saved()).toBe('FREQ=DAILY;UNTIL=20261111T055959Z');
  });

  it('shows the typed day, not the UTC digits, on a calendar that stores the end as a date', async () => {
    deviceIn('America/New_York');
    await open(single('2026-11-04T00:00:00.000Z', 'cal-work'));
    dailyThroughTenth(null);
    await pick('Chicago', 'Chicago');
    expect(endField('chicago').value).toBe('2026-11-10');
    expect(await saved()).toBe('FREQ=DAILY;UNTIL=20261111T055959Z');
  });

  it('keeps the day east of UTC moved west', async () => {
    // 3 November, 01:00 Berlin: 2 November, 19:00 New York.
    deviceInBerlin();
    await open(single('2026-11-03T00:00:00.000Z', 'cal-dav'));
    dailyThroughTenth(null);
    await pick('New York', 'New York');
    expect(endField('new york').value).toBe('2026-11-10');
    expect(await saved()).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
  });
});

describe("EventDialog → a file's series: its end is read once as a day (136a, 141a)", () => {
  /** A daily series at 19:00 New York from 2 November (or from `start`),
   *  ending `until`. With no UID, so no calendar is asked whether it holds it
   *  already. */
  const fromFile = (until: string, start = '2026-11-03T00:00:00Z'): ImportedEvent => ({
    title: 'Abendrunde',
    start,
    end: new Date(Date.parse(start) + 3_600_000).toISOString(),
    all_day: false,
    recurrence: { rrule: `FREQ=DAILY;UNTIL=${until}`, exceptions: [], tzid: 'America/New_York' },
    reminders: [],
    attendees: [],
  });

  interface Sent {
    start: string;
    end: string;
    recurrence: { rrule: string; tzid?: string };
  }

  /** Saves the new series and answers what it was created with. */
  async function createdSeries(): Promise<Sent> {
    fireEvent.click(screen.getByRole('button', { name: /^anlegen$/i }));
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));
    return (calls('create_event')[0][1] as { request: Sent }).request;
  }
  const created = async () => (await createdSeries()).recurrence.rrule;

  /** The last occurrence the views show of a series as it was saved. */
  const lastOccurrence = (series: Sent) => {
    const all = expandEvent(
      {
        id: 'x',
        start: series.start,
        end: series.end,
        all_day: false,
        recurrence: { exceptions: [], ...series.recurrence },
      },
      { start: new Date('2026-10-01T00:00:00Z'), end: new Date('2027-01-01T00:00:00Z') },
    );
    return new Date(all[all.length - 1].start).toISOString();
  };

  const intoCalDav = () =>
    fireEvent.change(screen.getByRole('combobox', { name: /kalender/i }), {
      target: { value: 'cal-dav' },
    });

  it.each([
    ['Outlook', '20261111T000000Z'],
    ['Google', '20261111T045959Z'],
  ])("shows a %s end's day on the series' clock and saves it through that day", async (_, until) => {
    deviceInBerlin();
    await open(null, fromFile(until));
    expect(endField('new york').value).toBe('2026-11-10');
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
  });

  it.each(['cal-work', 'cal-dav'])(
    'reads an end the guard alone reads as a cut as the day before it, into %s (140a)',
    async (calendarId) => {
      // `…T235959Z` one second before the 10 November occurrence: the file's
      // series ends on 9 November, as RFC 5545 and 105a read it.
      deviceInBerlin();
      await open(null, fromFile('20261110T235959Z'));
      fireEvent.change(screen.getByRole('combobox', { name: /kalender/i }), {
        target: { value: calendarId },
      });
      expect(endField('new york').value).toBe('2026-11-09');
      // Through 9 November, 19:00 New York.
      expect(await created()).toBe('FREQ=DAILY;UNTIL=20261110T045959Z');
    },
  );

  it("spells a day typed in the file's series on its clock", async () => {
    deviceInBerlin();
    await open(null, fromFile('20261111T045959Z'));
    fireEvent.change(endField('new york'), { target: { value: '2026-11-12' } });
    expect(endField('new york').value).toBe('2026-11-12');
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261113T045959Z');
  });

  it('keeps a typed day while the time moves to the blind spot, on CalDAV', async () => {
    // 10:00 New York, ending 20 November; moved to 19:00, ending 10 November.
    deviceInBerlin();
    await open(null, fromFile('20261120T235959Z', '2026-11-03T15:00:00Z'));
    intoCalDav();
    fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: '19:00' } });
    fireEvent.change(endField('new york'), { target: { value: '2026-11-10' } });
    expect(endField('new york').value).toBe('2026-11-10');
    const series = await createdSeries();
    expect(series.start).toBe('2026-11-04T00:00:00.000Z');
    expect(series.recurrence.rrule).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
    // 10 November, 19:00 New York.
    expect(lastOccurrence(series)).toBe('2026-11-11T00:00:00.000Z');
  });

  it("keeps the file's last day untouched while the time moves to the blind spot", async () => {
    // 10:00 New York through 10 November; moved to 19:00.
    deviceInBerlin();
    await open(null, fromFile('20261110T235959Z', '2026-11-03T15:00:00Z'));
    intoCalDav();
    expect(endField('new york').value).toBe('2026-11-10');
    fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: '19:00' } });
    expect(endField('new york').value).toBe('2026-11-10');
    const series = await createdSeries();
    expect(series.recurrence.rrule).toBe('FREQ=DAILY;UNTIL=20261111T045959Z');
    expect(lastOccurrence(series)).toBe('2026-11-11T00:00:00.000Z');
  });

  it('keeps the day a guarded end means while the time moves out of the blind spot', async () => {
    // 19:00 New York, guarded: through 9 November. Moved to 10:00, it still
    // ends on 9 November, not with one more occurrence on the 10th.
    deviceInBerlin();
    await open(null, fromFile('20261110T235959Z'));
    intoCalDav();
    expect(endField('new york').value).toBe('2026-11-09');
    fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: '10:00' } });
    expect(endField('new york').value).toBe('2026-11-09');
    const series = await createdSeries();
    expect(series.recurrence.rrule).toBe('FREQ=DAILY;UNTIL=20261109T235959Z');
    // 9 November, 10:00 New York.
    expect(lastOccurrence(series)).toBe('2026-11-09T15:00:00.000Z');
  });

  it('saves an untouched end of a zone this device cannot use: the day read is no change', async () => {
    // Daily 17:00 Arizona (00:00 UTC) through 10 November there, spelled at
    // midnight UTC as Outlook writes it. The form holds it as a day; that is
    // the rule the editor was filled with, so the save is not refused as a
    // changed rule (`unusableZoneRefusal`).
    deviceInBerlin();
    const Real = Intl.DateTimeFormat;
    vi.spyOn(Intl, 'DateTimeFormat').mockImplementation(function (
      locales?: string | string[],
      options?: Intl.DateTimeFormatOptions,
    ) {
      if (options?.timeZone === 'US/Arizona') {
        throw new RangeError('Invalid time zone specified: US/Arizona');
      }
      return new Real(locales, options);
    } as unknown as typeof Intl.DateTimeFormat);
    const file = fromFile('20261111T070000Z');
    await open(null, { ...file, recurrence: { ...file.recurrence!, tzid: 'US/Arizona' } });
    expect(input(/^ende-datum \((phoenix|arizona)\)$/i).value).toBe('2026-11-10');
    // Through 10 November 17:00 Arizona.
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261111T065959Z');
  });

  it('keeps the day when another zone is chosen, and spells it on that clock', async () => {
    deviceInBerlin();
    await open(null, fromFile('20261111T045959Z'));
    intoCalDav();
    await pick('Chicago', 'Chicago');
    expect(endField('chicago').value).toBe('2026-11-10');
    // Through 10 November 18:00 Chicago (11 November 00:00 UTC).
    expect(await created()).toBe('FREQ=DAILY;UNTIL=20261111T055959Z');
  });
});

describe("EventDialog → the end field reads its day on the series' zone, which its label names", () => {
  /** A daily CalDAV series on `tzid`'s clock from `start`, ending `until`. */
  const series = (start: string, until: string, tzid = 'America/New_York') =>
    ({
      id: 'ev-ny',
      calendar_id: 'cal-dav',
      title: 'Abendrunde',
      description: null,
      location: null,
      start,
      end: new Date(Date.parse(start) + 3_600_000).toISOString(),
      all_day: false,
      recurrence: { rrule: `FREQ=DAILY;UNTIL=${until}`, exceptions: [], tzid },
      color_label: null,
      reminders: [],
      attendees: [],
    }) as unknown as CalendarEvent;

  /** The last occurrence the views show of `ev`. */
  const lastOccurrence = (ev: CalendarEvent) => {
    const all = expandEvent(
      { id: 'x', start: ev.start, end: ev.end, all_day: ev.all_day, recurrence: ev.recurrence },
      { start: new Date('2026-10-01T00:00:00Z'), end: new Date('2027-01-01T00:00:00Z') },
    );
    return new Date(all[all.length - 1].start).toISOString();
  };

  /** Opens the occurrence of `stored` at `at` as from a reminder: the scope
   *  is asked in the form, and the other fields stand on the device's clock. */
  async function openFromReminder(stored: CalendarEvent, at: string) {
    seriesById.set(stored.id, stored);
    await open(occurrenceOfSeries(stored, at) as unknown as CalendarEvent);
    await settle();
  }

  /** Picks the whole series in the form, types `day` as its end on New York's
   *  clock and saves: the series as sent. */
  async function savedThrough(day: string): Promise<CalendarEvent> {
    fireEvent.click(screen.getByRole('radio', { name: /^ganze serie$/i }));
    await settle();
    fireEvent.change(endField('new york'), { target: { value: day } });
    expect(endField('new york').value).toBe(day);
    fireEvent.click(screen.getByRole('button', { name: /^speichern$/i }));
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    return (calls('update_event')[0][1] as { event: CalendarEvent }).event;
  }

  it("names the series' zone and reads a through-day end there, with the scope in the form", async () => {
    // 19:30 New York (01:30 in Berlin) through 10 November there, as
    // `until_through` writes it: the views' last occurrence is 10 November
    // 19:30 New York, 11 November in Berlin.
    deviceInBerlin();
    const stored = series('2026-11-03T00:30:00.000Z', '20261110T235959Z');
    expect(lastOccurrence(stored)).toBe('2026-11-11T00:30:00.000Z');
    await openFromReminder(stored, '2026-11-05T00:30:00.000Z');
    // The other fields stand on the device's clock and name none.
    expect(screen.getByLabelText(/^startzeit$/i)).toBeInTheDocument();
    expect(endField('new york').value).toBe('2026-11-10');
    const sent = await savedThrough('2026-11-12');
    expect(sent.recurrence?.tzid).toBe('America/New_York');
    expect(sent.recurrence?.rrule).toBe('FREQ=DAILY;UNTIL=20261112T235959Z');
    // 12 November, 19:30 New York: the day typed, on the clock named.
    expect(lastOccurrence(sent)).toBe('2026-11-13T00:30:00.000Z');
  });

  it("reads and spells an end at the blind spot on the series' zone, with the scope in the form", async () => {
    // 19:00 New York (01:00 in Berlin) through 10 November there. Read on
    // Berlin's clock the same end would be 11 November, and a typed day
    // would be spelled through Berlin's day.
    deviceInBerlin();
    const stored = series('2026-11-03T00:00:00.000Z', '20261111T045959Z');
    await openFromReminder(stored, '2026-11-05T00:00:00.000Z');
    expect(endField('new york').value).toBe('2026-11-10');
    const sent = await savedThrough('2026-11-12');
    expect(sent.recurrence?.rrule).toBe('FREQ=DAILY;UNTIL=20261113T045959Z');
    // 12 November, 19:00 New York.
    expect(lastOccurrence(sent)).toBe('2026-11-13T00:00:00.000Z');
  });

  /** Makes this device's `Intl` refuse US/Arizona, a name the zone picker
   *  never asks Intl about, so no formatter is cached. */
  function refuseArizona() {
    const Real = Intl.DateTimeFormat;
    vi.spyOn(Intl, 'DateTimeFormat').mockImplementation(function (
      locales?: string | string[],
      options?: Intl.DateTimeFormatOptions,
    ) {
      if (options?.timeZone === 'US/Arizona') {
        throw new RangeError('Invalid time zone specified: US/Arizona');
      }
      return new Real(locales, options);
    } as unknown as typeof Intl.DateTimeFormat);
  }

  /** The end field of a series on Arizona's clock, by the city the list
   *  names it with. */
  const arizonaEndField = () => input(/^ende-datum \((phoenix|arizona)\)$/i);

  it.each([
    // Through 10 November as `until_through` writes it there, and as an
    // instant, the last second of that day in Arizona.
    ['a through-day end', '20261110T235959Z'],
    ['an instant end', '20261111T065959Z'],
  ])('reads %s on the stored zone, named, for a zone this device cannot use', async (_, until) => {
    // Daily 17:30 Arizona (00:30 UTC): the views show it on UTC here, its
    // last occurrence on 11 November 00:30 UTC, which is 10 November in
    // Arizona.
    deviceInBerlin();
    refuseArizona();
    const stored = { ...series('2026-11-03T00:30:00.000Z', until, 'US/Arizona'), id: 'ev-az' };
    expect(lastOccurrence(stored)).toBe('2026-11-11T00:30:00.000Z');
    await open(stored);
    expect(input(/^startzeit \(utc\)$/i).value).toBe('00:30');
    expect(arizonaEndField().value).toBe('2026-11-10');
    // A typed day reads back as typed. Saving a changed rule is refused for
    // such a zone (`unusableZoneRefusal`), so nothing is written.
    fireEvent.change(arizonaEndField(), { target: { value: '2026-11-12' } });
    expect(arizonaEndField().value).toBe('2026-11-12');
  });
});
