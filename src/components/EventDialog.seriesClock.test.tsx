import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import { DEFAULT_RECURRENCE_CAPABILITIES } from '@aperio/shared';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Times typed on the series' clock (DESIGN-series-time-zone.md, stage 12).
 *
 * A series' start and end are typed and shown on the clock it repeats on: its
 * zone, or UTC. Where that is not the device's clock, the labels name it and
 * the times on this device follow the end time as a read-only field. The
 * repeat field moves the fields between the clocks silently and says it once
 * when it is left (134a); a time the series' clock skips is said under its
 * field and refuses the save; a new series is created with the zone the form
 * gives it, never with a device stamp; and "this and all following" loads its
 * series when it opens and never falls back to the device's clock (21a, 135a).
 *
 * The device is Berlin here, whatever clock the machine runs on.
 */

const { invokeMock, onFile, announced, announce } = vi.hoisted(() => {
  const announced: string[] = [];
  const announce = (message: string) => {
    announced.push(message);
  };
  /** What `get_event_by_id` answers: the series, or a failure. */
  const onFile: { series: unknown; fails: boolean } = { series: null, fails: false };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'create_event') {
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'created-1' });
    }
    if (command === 'get_event_by_id') {
      return onFile.fails
        ? Promise.reject({ code: 'network', message: 'offline' })
        : Promise.resolve(onFile.series);
    }
    if (command === 'get_series_rows') {
      return Promise.resolve({ rows: [], reach: { kind: 'complete' } });
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, onFile, announced, announce };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  {
    id: 'cal-work',
    name: 'Arbeit',
    read_only: false,
    account_id: 'acc-caldav',
    recurrence_capabilities: { ...DEFAULT_RECURRENCE_CAPABILITIES },
  } as unknown as Calendar,
];

/** Mondays at 09:00 in New York from 15 June: 15:00 in Berlin. */
const NEW_YORK: CalendarEvent = {
  id: 'ev-ny',
  calendar_id: 'cal-work',
  title: 'Standup',
  description: null,
  location: null,
  start: '2026-06-15T13:00:00.000Z',
  end: '2026-06-15T14:00:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], tzid: 'America/New_York' },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work']),
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
  vi.useRealTimers();
  document.body.innerHTML = '';
  invokeMock.mockClear();
  onFile.series = null;
  onFile.fails = false;
  announced.length = 0;
  (CALENDARS[0] as { recurrence_capabilities?: unknown }).recurrence_capabilities = {
    ...DEFAULT_RECURRENCE_CAPABILITIES,
  };
  vi.restoreAllMocks();
});

/** Pretend the device is in Berlin, whatever zone the test machine is in. */
function deviceInBerlin() {
  const real = new Intl.DateTimeFormat().resolvedOptions();
  vi.spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions').mockReturnValue({
    ...real,
    timeZone: 'Europe/Berlin',
  });
}

const calls = (command: string) => invokeMock.mock.calls.filter((call) => call[0] === command);

async function open(
  event: CalendarEvent | null,
  initialScope?: 'series' | 'this_and_future' | 'occurrence',
) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} initialScope={initialScope} />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

const input = (label: RegExp) => screen.getByLabelText(label) as HTMLInputElement;
const repeatSelect = () =>
  screen.getByRole('combobox', { name: /^wiederholung$/i }) as HTMLSelectElement;
const save = (name = /^speichern$/i) => fireEvent.click(screen.getByRole('button', { name }));

describe('EventDialog → the labels name the series clock', () => {
  it('reads a series without a zone as "Startzeit (UTC)"', async () => {
    deviceInBerlin();
    await open({
      ...NEW_YORK,
      start: '2026-06-15T22:30:00.000Z',
      end: '2026-06-15T23:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [] },
    } as unknown as CalendarEvent);
    expect(input(/^startzeit \(utc\)$/i).value).toBe('22:30');
    expect(input(/^startdatum \(utc\)$/i).value).toBe('2026-06-15');
    expect(input(/^endzeit \(utc\)$/i).value).toBe('23:30');
  });

  it('says the times on this device after the end time, as a read-only text field', async () => {
    deviceInBerlin();
    await open(NEW_YORK);
    expect(input(/^startzeit \(new york\)$/i).value).toBe('09:00');
    const hint = screen.getByRole('textbox', { name: /^auf diesem gerät \(berlin\)$/i });
    // A field with its own tab stop, never role=note (NVDA reads only
    // "Anmerkung" there).
    expect(hint.tagName).toBe('TEXTAREA');
    expect(hint.getAttribute('aria-readonly')).toBe('true');
    expect((hint as HTMLTextAreaElement).value).toBe('Montag, 15. Juni, 15:00 bis 16:00');
  });

  it('keeps the plain labels and no hint where the series runs on the device clock', async () => {
    deviceInBerlin();
    await open({
      ...NEW_YORK,
      start: '2026-06-15T07:00:00.000Z',
      end: '2026-06-15T08:00:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], tzid: 'Europe/Berlin' },
    } as unknown as CalendarEvent);
    expect(input(/^startzeit$/i).value).toBe('09:00');
    expect(screen.queryByRole('textbox', { name: /auf diesem gerät/i })).toBeNull();
  });

  it('saves an untouched series with its stored instants and zone', async () => {
    deviceInBerlin();
    await open(NEW_YORK);
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = (calls('update_event')[0][1] as { event: CalendarEvent }).event;
    expect(sent.start).toBe(NEW_YORK.start);
    expect(sent.end).toBe(NEW_YORK.end);
    expect(sent.recurrence).toEqual(NEW_YORK.recurrence);
  });

  it('reads a typed time on the series clock', async () => {
    deviceInBerlin();
    await open(NEW_YORK);
    fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: '10:30' } });
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = (calls('update_event')[0][1] as { event: CalendarEvent }).event;
    // 10:30 in New York (EDT) on 15 June, an hour and a half on, the end along.
    expect(sent.start).toBe('2026-06-15T14:30:00.000Z');
    expect(sent.end).toBe('2026-06-15T15:30:00.000Z');
    expect(sent.recurrence?.tzid).toBe('America/New_York');
  });
});

describe('EventDialog → the repeat field moves the fields silently and says it once (134a)', () => {
  it('announces nothing while arrowing, and once when the field is left', async () => {
    deviceInBerlin();
    await open(NEW_YORK);
    vi.useFakeTimers();
    const select = repeatSelect();
    select.focus();
    // Each arrow in the native select is a change.
    fireEvent.change(select, { target: { value: 'NONE' } });
    // On the device's clock at once: the labels lose their clock.
    expect(input(/^startzeit$/i).value).toBe(
      new Intl.DateTimeFormat('de', { hour: '2-digit', minute: '2-digit' }).format(
        new Date(NEW_YORK.start),
      ),
    );
    fireEvent.change(repeatSelect(), { target: { value: 'DAILY' } });
    expect(input(/^startzeit \(new york\)$/i).value).toBe('09:00');
    fireEvent.change(repeatSelect(), { target: { value: 'NONE' } });
    vi.advanceTimersByTime(5000);
    expect(announced).toEqual([]);

    repeatSelect().blur();
    expect(announced).toHaveLength(1);
    expect(announced[0]).toMatch(
      /^Keine Wiederholung\. Die Zeiten stehen jetzt auf der Uhr dieses Geräts: Montag, 15\. Juni, 15:00 bis 16:00\.$/,
    );
    vi.advanceTimersByTime(5000);
    expect(announced).toHaveLength(1);
  });

  it('says nothing when the field is left on the clock it was entered on', async () => {
    deviceInBerlin();
    await open(NEW_YORK);
    const select = repeatSelect();
    select.focus();
    fireEvent.change(select, { target: { value: 'NONE' } });
    fireEvent.change(repeatSelect(), { target: { value: 'WEEKLY' } });
    repeatSelect().blur();
    expect(announced).toEqual([]);
    // Back on the stored clock, holding the stored instant.
    expect(input(/^startzeit \(new york\)$/i).value).toBe('09:00');
  });
});

describe('EventDialog → a time the series clock skips', () => {
  it('is said under the time field, marks it invalid and refuses the save', async () => {
    deviceInBerlin();
    // Sundays at 09:00 in New York, from 8 March 2026: the clocks go forward
    // at 02:00 that night.
    await open({
      ...NEW_YORK,
      start: '2026-03-08T13:00:00.000Z',
      end: '2026-03-08T14:00:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [], tzid: 'America/New_York' },
    } as unknown as CalendarEvent);
    const time = input(/^startzeit \(new york\)$/i);
    fireEvent.change(time, { target: { value: '02:30' } });

    const message = /^02:30 gibt es am 8\. März 2026 in New York nicht: Die Uhren werden vorgestellt\. Wähle eine andere Uhrzeit\.$/;
    const shown = screen.getByText(message);
    const field = input(/^startzeit \(new york\)$/i);
    expect(field.getAttribute('aria-invalid')).toBe('true');
    expect(field.getAttribute('aria-describedby')).toBe(shown.id);

    save();
    expect((await screen.findByRole('alert')).textContent).toMatch(message);
    expect(calls('update_event')).toHaveLength(0);
  });
});

describe('EventDialog → a new series', () => {
  it('is created with the zone the form gives it, never a device stamp', async () => {
    deviceInBerlin();
    // A calendar that cannot store Berlin: the new series starts on UTC
    // (22a), and saying so follows the end time.
    (CALENDARS[0] as { recurrence_capabilities?: unknown }).recurrence_capabilities = {
      ...DEFAULT_RECURRENCE_CAPABILITIES,
      time_zones: ['America/New_York'],
    };
    await open(null);
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
      target: { value: 'Neu' },
    });
    fireEvent.change(repeatSelect(), { target: { value: 'WEEKLY' } });
    expect(
      screen.getByDisplayValue(
        'Dieser Kalender kann die Zeitzone Berlin nicht speichern; die Serie steht auf UTC.',
      ).getAttribute('aria-readonly'),
    ).toBe('true');
    save(/^anlegen$/i);
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));
    const request = (calls('create_event')[0][1] as {
      request: { recurrence: Record<string, unknown> };
    }).request;
    expect(request.recurrence.rrule).toMatch(/^FREQ=WEEKLY/);
    // Stamped with the device's zone it would carry `tzid: Europe/Berlin`.
    expect(request.recurrence.tzid ?? null).toBeNull();
  });

  it("gets the device's zone, written canonically, where the calendar stores it", async () => {
    deviceInBerlin();
    await open(null);
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
      target: { value: 'Neu' },
    });
    fireEvent.change(repeatSelect(), { target: { value: 'DAILY' } });
    save(/^anlegen$/i);
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));
    const request = (calls('create_event')[0][1] as {
      request: { recurrence: Record<string, unknown> };
    }).request;
    expect(request.recurrence.tzid).toBe('Europe/Berlin');
  });
});

describe('EventDialog → "this and all following" stands on the series clock (21a, 135a)', () => {
  /** An occurrence as the views expand it, carrying the series' rule. */
  const occurrence = (tzid: string | null) =>
    ({
      ...NEW_YORK,
      id: 'ev-ny@2026-07-06T13:00:00.000Z',
      series_id: 'ev-ny',
      occurrence_start: '2026-07-06T13:00:00.000Z',
      start: '2026-07-06T13:00:00.000Z',
      end: '2026-07-06T14:00:00.000Z',
      recurrence: { ...NEW_YORK.recurrence!, tzid },
    }) as unknown as CalendarEvent;

  it('loads the series when it opens, and names its zone where the choice would be', async () => {
    deviceInBerlin();
    onFile.series = NEW_YORK;
    // The row says UTC; the series it belongs to says New York.
    await open(occurrence(null), 'this_and_future');
    expect(calls('get_event_by_id')).not.toHaveLength(0);
    await screen.findByLabelText(/^startzeit \(new york\)$/i);
    expect(input(/^startzeit \(new york\)$/i).value).toBe('09:00');
    const hint = screen.getByRole('textbox', { name: /^zeitzone der serie$/i });
    expect(hint.getAttribute('aria-readonly')).toBe('true');
    expect((hint as HTMLTextAreaElement).value).toBe(
      'New York. Ändern lässt sie sich, wenn du die ganze Serie bearbeitest.',
    );
  });

  it('takes the loaded series on UTC over the zone the row still carries', async () => {
    deviceInBerlin();
    // Switched to UTC on another device; the cached row still says New York.
    onFile.series = { ...NEW_YORK, recurrence: { ...NEW_YORK.recurrence!, tzid: null } };
    await open(occurrence('America/New_York'), 'this_and_future');
    await screen.findByLabelText(/^startzeit \(utc\)$/i);
    expect(input(/^startzeit \(utc\)$/i).value).toBe('13:00');
    expect(
      (screen.getByRole('textbox', { name: /^zeitzone der serie$/i }) as HTMLTextAreaElement).value,
    ).toBe('UTC. Ändern lässt sie sich, wenn du die ganze Serie bearbeitest.');
  });

  it('says a series that cannot be loaded, refuses the save, and never falls back to the device clock', async () => {
    deviceInBerlin();
    onFile.fails = true;
    await open(occurrence('America/New_York'), 'this_and_future');
    const failure = 'Die Serie konnte nicht geladen werden. Versuche es noch einmal, oder bearbeite nur diesen Termin.';
    const field = await screen.findByDisplayValue(failure);
    expect(field.getAttribute('aria-readonly')).toBe('true');
    expect(announced).toContain(failure);
    // Still on the clock the occurrence carries, not the device's.
    expect(input(/^startzeit \(new york\)$/i).value).toBe('09:00');
    expect(screen.queryByLabelText(/^startzeit$/i)).toBeNull();

    save();
    expect((await screen.findByRole('alert')).textContent).toBe(failure);
    expect(calls('create_event')).toHaveLength(0);
    expect(calls('update_event')).toHaveLength(0);
  });

  it('takes a time typed on the series clock from the first occurrence, though the device reads it as before', async () => {
    deviceInBerlin();
    // Tuesdays at 10:00 New York from 10 March 2026, when US summer time has
    // begun and the EU's has not: 15:00 in Berlin, as 09:00 New York is on
    // 31 March. On a machine in Berlin the device's reading does not change.
    const march = {
      ...NEW_YORK,
      id: 'ev-mar',
      start: '2026-03-10T14:00:00.000Z',
      end: '2026-03-10T15:00:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=TU', exceptions: [], tzid: 'America/New_York' },
    } as unknown as CalendarEvent;
    onFile.series = march;
    await open(
      {
        ...march,
        id: 'ev-mar@2026-03-10T14:00:00.000Z',
        series_id: 'ev-mar',
        occurrence_start: '2026-03-10T14:00:00.000Z',
      } as unknown as CalendarEvent,
      'this_and_future',
    );
    await screen.findByLabelText(/^startzeit \(new york\)$/i);
    fireEvent.change(input(/^startdatum \(new york\)$/i), { target: { value: '2026-03-31' } });
    fireEvent.change(input(/^startzeit \(new york\)$/i), { target: { value: '09:00' } });
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = (calls('update_event')[0][1] as { event: CalendarEvent }).event;
    expect(sent.start).toBe('2026-03-31T13:00:00.000Z');
    expect(sent.end).toBe('2026-03-31T14:00:00.000Z');
  });

  it('opens a series that no longer repeats without the failure, and the save says why', async () => {
    deviceInBerlin();
    onFile.series = { ...NEW_YORK, recurrence: null };
    await open(occurrence('America/New_York'), 'this_and_future');
    await waitFor(() => expect(calls('get_event_by_id')).not.toHaveLength(0));
    expect(screen.queryByDisplayValue(/konnte nicht geladen werden/)).toBeNull();
    save();
    await screen.findByText(/wiederholt sich nicht mehr/i);
    expect(calls('create_event')).toHaveLength(0);
  });
});
