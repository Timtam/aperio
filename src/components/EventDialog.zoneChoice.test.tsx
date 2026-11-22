import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { DEFAULT_RECURRENCE_CAPABILITIES, fieldInstant, seriesClockChange } from '@aperio/shared';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Choosing a series' zone in the desktop editor (DESIGN-series-time-zone.md,
 * stage 12, "Die Zone wechseln" and "Die Wochentag-Frage (15a)", 138a).
 *
 * A button after the repeat block, named with the zone in effect, opens the
 * picker. A zone picked moves the fields, the rule and the exceptions as the
 * core answers; where it asks, the weekday question opens once the picker has
 * closed, with the focus on "Zeitzone nicht ändern". Afterwards the focus is
 * on the button and one announcement says what happened. The whole-series
 * save is measured against the series as the answer leaves it, so the answer
 * is applied once; and a save that changed the zone offers no carry to the
 * group's copies (137a).
 *
 * The device is Berlin here, whatever clock the machine runs on.
 */

const { invokeMock, onFile, answers, announced, announce, openEventGroupCarry } = vi.hoisted(() => {
  const announced: string[] = [];
  const announce = (message: string) => {
    announced.push(message);
  };
  const onFile: { groups: unknown[] } = { groups: [] };
  /** What each `update_event` answers, in order; then the event as sent. */
  const answers: Array<(event: unknown) => Promise<unknown>> = [];
  const openEventGroupCarry = vi.fn();
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      const event = (payload as { event: unknown }).event;
      const next = answers.shift();
      return next ? next(event) : Promise.resolve(event);
    }
    if (command === 'create_event') {
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'created-1' });
    }
    if (command === 'get_series_rows') {
      return Promise.resolve({ rows: [], reach: { kind: 'complete' } });
    }
    if (command === 'event_groups_for_events') {
      return Promise.resolve(onFile.groups);
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, onFile, answers, announced, announce, openEventGroupCarry };
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

/** Sunday 22 November 2026, 23:30 UTC: Monday 00:30 in Berlin. */
const SUNDAY_LATE = '2026-11-22T23:30:00.000Z';

/** A series without a zone, Sundays and Wednesdays at 23:30 UTC: on Berlin's
 *  clock its start falls on Monday, and the core asks. */
const UTC_SERIES: CalendarEvent = {
  id: 'ev-utc',
  calendar_id: 'cal-work',
  title: 'Abendrunde',
  description: null,
  location: null,
  start: SUNDAY_LATE,
  end: '2026-11-23T00:30:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=SU,WE', exceptions: [] },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

const withRule = (rrule: string, exceptions: string[] = []): CalendarEvent =>
  ({ ...UTC_SERIES, recurrence: { rrule, exceptions } }) as unknown as CalendarEvent;

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry };
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
  openEventGroupCarry.mockClear();
  onFile.groups = [];
  answers.length = 0;
  announced.length = 0;
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
const updatesSent = () =>
  calls('update_event').map((call) => (call[1] as { event: CalendarEvent }).event);

async function open(event: CalendarEvent | null) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

const input = (label: RegExp) => screen.getByLabelText(label) as HTMLInputElement;
const zoneButton = () => screen.getByRole('button', { name: /^zeitzone der serie: /i });
const save = (name = /^speichern$/i) => fireEvent.click(screen.getByRole('button', { name }));

/** Opens the picker from the zone button and picks the first row whose text
 *  starts with `row`, after searching for `query` (none: the rows above the
 *  list). */
async function pick(row: string, query = '') {
  // As a key or a click would, the button has the focus first: the picker
  // gives it back there when it closes.
  zoneButton().focus();
  fireEvent.click(zoneButton());
  const picker = await screen.findByRole('dialog', { name: /^zeitzone der serie$/i });
  if (query !== '') {
    fireEvent.change(within(picker).getByRole('combobox', { name: /^zeitzone suchen$/i }), {
      target: { value: query },
    });
  }
  const option = within(picker)
    .getAllByRole('option')
    .find((candidate) => candidate.textContent?.startsWith(row));
  if (!option) throw new Error(`no row ${row}`);
  await act(async () => {
    fireEvent.mouseDown(option);
  });
}

/** The weekday question, once it opened with the focus on "Zeitzone nicht
 *  ändern". */
async function question() {
  const dialog = await screen.findByRole('dialog', { name: 'Welcher Wochentag soll gelten?' });
  const keep = within(dialog).getByRole('button', { name: 'Zeitzone nicht ändern' });
  await waitFor(() => expect(document.activeElement).toBe(keep));
  return { dialog, keep };
}

/** The question's own buttons, without the header's close button. */
const answerButtons = (dialog: HTMLElement) =>
  within(dialog)
    .getAllByRole('button')
    .filter((button) => !button.classList.contains('modal__close'));

/** Settles the focus moves the dialogs make in microtasks. */
const settle = () => act(async () => {});

describe('EventDialog → the zone button', () => {
  it('names the zone in effect and opens a dialog', async () => {
    deviceInBerlin();
    await open(UTC_SERIES);
    const button = zoneButton();
    expect(button).toHaveAccessibleName('Zeitzone der Serie: UTC (ohne Sommerzeit)');
    expect(button.getAttribute('aria-haspopup')).toBe('dialog');
    fireEvent.click(button);
    expect(await screen.findByRole('dialog', { name: /^zeitzone der serie$/i })).toBeTruthy();
  });

  it('is not there for a series that only repeats once the form says so', async () => {
    deviceInBerlin();
    await open({ ...UTC_SERIES, recurrence: null } as unknown as CalendarEvent);
    expect(screen.queryByRole('button', { name: /^zeitzone der serie: /i })).toBeNull();
  });
});

describe('EventDialog → the weekday question (138a)', () => {
  it('opens after the picker with the focus on "Zeitzone nicht ändern", the answers after it', async () => {
    deviceInBerlin();
    await open(UTC_SERIES);
    await pick('Berlin', 'Berlin');
    const { dialog, keep } = await question();
    expect(document.activeElement).not.toBe(document.body);
    // The picker is gone; only the question is asked.
    expect(screen.queryByRole('dialog', { name: /^zeitzone der serie$/i })).toBeNull();
    expect(answerButtons(dialog).map((button) => button.textContent)).toEqual([
      'Zeitzone nicht ändern',
      'Jeden Mittwoch und Sonntag, wie die Regel sagt (Termine rücken einen Tag vor)',
      'Jeden Montag und Donnerstag, wo die Termine jetzt liegen',
    ]);
    // The message is read with whichever button has the focus.
    for (const button of answerButtons(dialog)) {
      expect(button).toHaveAccessibleDescription(
        'In Berlin fällt der Beginn auf Montag, 23. November, 00:30; die Regel sagt jeden Mittwoch und Sonntag.',
      );
    }
    expect(keep.className).not.toMatch(/danger|primary/);
    expect(announced).toEqual([]);
  });

  it('takes an answer, leaves the focus on the zone button and says it once', async () => {
    deviceInBerlin();
    await open(UTC_SERIES);
    await pick('Berlin', 'Berlin');
    const { dialog } = await question();
    fireEvent.click(within(dialog).getByRole('button', { name: /wo die termine jetzt liegen$/i }));
    await settle();
    await waitFor(() => expect(document.activeElement).toBe(zoneButton()));
    expect(zoneButton()).toHaveAccessibleName('Zeitzone der Serie: Berlin, Europa, UTC+01:00');
    expect(announced).toHaveLength(1);
    expect(announced[0]).toMatch(
      /^Zeitzone der Serie: Berlin\. Die Serie beginnt zum selben Zeitpunkt: Montag, 23\. November, 00:30 .*auf dieser Uhr\. Wiederholt sich jeden Montag und Donnerstag\.$/,
    );
    // The fields stand on Berlin's clock, the device's: plain labels.
    expect(input(/^startdatum$/i).value).toBe('2026-11-23');
    expect(input(/^startzeit$/i).value).toBe('00:30');
  });

  it('moves the start where the rule says, and says so', async () => {
    deviceInBerlin();
    await open(UTC_SERIES);
    await pick('Berlin', 'Berlin');
    const { dialog } = await question();
    fireEvent.click(within(dialog).getByRole('button', { name: /wie die regel sagt/i }));
    await settle();
    await waitFor(() => expect(document.activeElement).toBe(zoneButton()));
    expect(announced).toEqual([
      'Zeitzone der Serie: Berlin. Die Serie beginnt jetzt am Sonntag, 22. November, 00:30 auf dieser Uhr.',
    ]);
    expect(input(/^startdatum$/i).value).toBe('2026-11-22');
    expect(input(/^startzeit$/i).value).toBe('00:30');
  });

  it.each([
    ['the keep button', (keep: HTMLElement) => fireEvent.click(keep)],
    ['Escape', (keep: HTMLElement) => fireEvent.keyDown(keep, { key: 'Escape' })],
  ])('changes nothing with %s, and says so', async (_how, act_) => {
    deviceInBerlin();
    await open(UTC_SERIES);
    await pick('Berlin', 'Berlin');
    const { keep } = await question();
    act_(keep);
    await settle();
    await waitFor(() => expect(document.activeElement).toBe(zoneButton()));
    expect(screen.queryByRole('dialog', { name: 'Welcher Wochentag soll gelten?' })).toBeNull();
    expect(announced).toEqual(['Zeitzone der Serie nicht geändert: UTC (ohne Sommerzeit)']);
    expect(zoneButton()).toHaveAccessibleName('Zeitzone der Serie: UTC (ohne Sommerzeit)');
    expect(input(/^startzeit \(utc\)$/i).value).toBe('23:30');
  });

  it('has no button for an answer the core refuses, and says why', async () => {
    deviceInBerlin();
    // The fourth Sunday: where the events fall now cannot be named on
    // Berlin's clock (ordinal_weekday).
    await open(withRule('FREQ=MONTHLY;BYDAY=4SU'));
    await pick('Berlin', 'Berlin');
    const { dialog } = await question();
    const buttons = answerButtons(dialog).map((button) => button.textContent);
    expect(buttons).toHaveLength(2);
    expect(buttons[0]).toBe('Zeitzone nicht ändern');
    expect(buttons[1]).toMatch(/wie die Regel sagt/);
    expect(dialog.textContent).toMatch(/Die Tage, wo die Termine jetzt liegen, gehen nicht: /);
  });

  it('refuses a change no answer can carry out, says why, and keeps the zone', async () => {
    deviceInBerlin();
    // A rule with its own hours (time_of_day): neither answer can be written.
    await open(withRule('FREQ=DAILY;BYHOUR=23'));
    await pick('Berlin', 'Berlin');
    await settle();
    await waitFor(() => expect(document.activeElement).toBe(zoneButton()));
    expect(screen.queryByRole('dialog', { name: 'Welcher Wochentag soll gelten?' })).toBeNull();
    expect(announced).toEqual([
      'Zeitzone der Serie nicht geändert: ihre Regel legt eigene Uhrzeiten fest.',
    ]);
    expect(zoneButton()).toHaveAccessibleName('Zeitzone der Serie: UTC (ohne Sommerzeit)');
    expect(input(/^startzeit \(utc\)$/i).value).toBe('23:30');
  });
});

describe('EventDialog → the zone in effect chosen again', () => {
  it('changes nothing, says so, and saves a stored Europe/Kiev as it is spelled', async () => {
    deviceInBerlin();
    await open({
      ...UTC_SERIES,
      start: '2026-11-23T07:00:00.000Z',
      end: '2026-11-23T08:00:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], tzid: 'Europe/Kiev' },
    } as unknown as CalendarEvent);
    await pick('Aktuell');
    await settle();
    await waitFor(() => expect(document.activeElement).toBe(zoneButton()));
    expect(announced).toEqual([
      'Zeitzone der Serie nicht geändert: Europe/Kiev (nicht in Aperios Liste)',
    ]);
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    expect(updatesSent()[0].recurrence).toEqual({
      rrule: 'FREQ=WEEKLY;BYDAY=MO',
      exceptions: [],
      tzid: 'Europe/Kiev',
    });
  });
});

describe('EventDialog → the whole-series save after a zone answer (question 12)', () => {
  const DELETED = '2026-11-25T23:30:00.000Z';
  const coreAnswer = (which: 'asNow' | 'asRule') => {
    const change = seriesClockChange({
      rrule: 'FREQ=WEEKLY;BYDAY=SU,WE',
      start: SUNDAY_LATE,
      exceptions: [DELETED],
      fromTzid: null,
      toTzid: 'Europe/Berlin',
    });
    const answer = change?.[which];
    if (answer?.outcome !== 'changed') throw new Error('the core gave no answer');
    return answer;
  };

  async function answered(name: RegExp) {
    await open(withRule('FREQ=WEEKLY;BYDAY=SU,WE', [DELETED]));
    await pick('Berlin', 'Berlin');
    const { dialog } = await question();
    fireEvent.click(within(dialog).getByRole('button', { name }));
    await settle();
  }

  it('writes the new zone, the rewritten rule and the moved exceptions', async () => {
    deviceInBerlin();
    await answered(/wo die termine jetzt liegen$/i);
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = updatesSent()[0];
    const answer = coreAnswer('asNow');
    expect(Date.parse(sent.start)).toBe(Date.parse(SUNDAY_LATE));
    expect(sent.recurrence).toEqual({
      rrule: answer.rrule,
      exceptions: answer.exceptions,
      tzid: 'Europe/Berlin',
    });
  });

  it('writes a start the rule moved once, with its exceptions', async () => {
    deviceInBerlin();
    await answered(/wie die regel sagt/i);
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = updatesSent()[0];
    const answer = coreAnswer('asRule');
    expect(Date.parse(sent.start)).toBe(Date.parse(answer.start));
    expect(Date.parse(sent.end) - Date.parse(sent.start)).toBe(3_600_000);
    expect(sent.recurrence).toEqual({
      rrule: 'FREQ=WEEKLY;BYDAY=SU,WE',
      exceptions: answer.exceptions,
      tzid: 'Europe/Berlin',
    });
  });

  it('moves the rule from the answer, not from the stored start, when the date moves too', async () => {
    deviceInBerlin();
    await open(UTC_SERIES);
    await pick('Berlin', 'Berlin');
    const { dialog } = await question();
    fireEvent.click(within(dialog).getByRole('button', { name: /wie die regel sagt/i }));
    await settle();
    // A week later on Berlin's clock: Sunday 29 November, 00:30.
    expect(input(/^startdatum$/i).value).toBe('2026-11-22');
    fireEvent.change(input(/^startdatum$/i), { target: { value: '2026-11-29' } });
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = updatesSent()[0];
    expect(sent.start).toBe('2026-11-28T23:30:00.000Z');
    // Moved again from the stored start, the rule would have moved by six
    // days on UTC's clock, to Saturdays and Tuesdays.
    expect(sent.recurrence?.rrule).toBe('FREQ=WEEKLY;BYDAY=SU,WE');
    expect(sent.recurrence?.tzid).toBe('Europe/Berlin');
  });

  it('asks to save a start changed before the choice first, and opens nothing', async () => {
    deviceInBerlin();
    await open(UTC_SERIES);
    fireEvent.change(input(/^startdatum \(utc\)$/i), { target: { value: '2026-11-29' } });
    zoneButton().focus();
    fireEvent.click(zoneButton());
    await settle();
    expect(screen.queryByRole('dialog', { name: /^zeitzone der serie$/i })).toBeNull();
    expect(announced).toContain(
      'Zeitzone der Serie nicht geändert: Speichere zuerst den geänderten Beginn, dann wähle die Zeitzone.',
    );
    expect(document.activeElement).toBe(zoneButton());
  });

  it('asks to save a series that opened all-day and got times first', async () => {
    deviceInBerlin();
    // Mondays all day from 23 November, on the device's days.
    const allDay = {
      ...UTC_SERIES,
      start: new Date(2026, 10, 23).toISOString(),
      end: new Date(2026, 10, 24).toISOString(),
      all_day: true,
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [new Date(2026, 10, 30).toISOString()] },
    } as unknown as CalendarEvent;
    await open(allDay);
    fireEvent.click(screen.getByRole('checkbox', { name: /ganztägig/i }));
    zoneButton().focus();
    fireEvent.click(zoneButton());
    await settle();
    expect(screen.queryByRole('dialog', { name: /^zeitzone der serie$/i })).toBeNull();
    expect(announced).toContain(
      'Zeitzone der Serie nicht geändert: Speichere zuerst den geänderten Beginn, dann wähle die Zeitzone.',
    );
  });

  it('keeps the deletions where all-day is switched on after an answer', async () => {
    deviceInBerlin();
    // Mondays at 09:00 in Berlin, two of them deleted, one in winter and one
    // in summer.
    const berlin = {
      ...UTC_SERIES,
      start: '2026-11-23T08:00:00.000Z',
      end: '2026-11-23T09:00:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=MO',
        exceptions: ['2026-12-07T08:00:00.000Z', '2027-07-05T07:00:00.000Z'],
        tzid: 'Europe/Berlin',
      },
    } as unknown as CalendarEvent;
    await open(berlin);
    await pick('New York', 'new york');
    await settle();
    fireEvent.click(screen.getByRole('checkbox', { name: /ganztägig/i }));
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    const sent = updatesSent()[0];
    expect(sent.all_day).toBe(true);
    // Both deleted Mondays stay deleted, as without the zone choice.
    expect(sent.recurrence?.exceptions).toHaveLength(2);
  });

  it('goes through the question about lost occurrences where the calendar would drop them', async () => {
    deviceInBerlin();
    answers.push(() =>
      Promise.reject({ code: 'forbidden', message: 'exceptions-would-be-lost: slot:1:0' }),
    );
    await answered(/wo die termine jetzt liegen$/i);
    save();
    const loss = await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
    fireEvent.click(within(loss).getByRole('button', { name: /trotzdem speichern/i }));
    await waitFor(() => expect(calls('update_event')).toHaveLength(2));
    const [first, second] = updatesSent();
    expect(first.accepts_exception_loss).toBeFalsy();
    expect(second.accepts_exception_loss).toBe(true);
    expect(second.recurrence?.tzid).toBe('Europe/Berlin');
  });
});

describe('EventDialog → no carry to the group after a zone change (137a)', () => {
  const inGroup = () => {
    onFile.groups = [
      {
        id: 'g1',
        created_at: '2026-06-01T00:00:00Z',
        updated_at: '2026-06-01T00:00:00Z',
        members: [
          { calendar_id: 'cal-work', event_id: 'ev-utc', title: 'Abendrunde', starts_at: SUNDAY_LATE, added_at: '2026-06-01T00:00:00Z' },
          { calendar_id: 'cal-work', event_id: 'ev-copy', title: 'Abendrunde', starts_at: SUNDAY_LATE, added_at: '2026-06-01T00:00:01Z' },
        ],
      },
    ];
  };

  it('offers the carry for a moved start without a zone change', async () => {
    deviceInBerlin();
    inGroup();
    await open(UTC_SERIES);
    fireEvent.change(input(/^startzeit \(utc\)$/i), { target: { value: '22:30' } });
    save();
    await waitFor(() => expect(openEventGroupCarry).toHaveBeenCalledTimes(1));
  });

  it('offers none where the answer moved the start to another zone', async () => {
    deviceInBerlin();
    inGroup();
    await open(UTC_SERIES);
    await pick('Berlin', 'Berlin');
    const { dialog } = await question();
    fireEvent.click(within(dialog).getByRole('button', { name: /wie die regel sagt/i }));
    await settle();
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));
    await waitFor(() => expect(announced.at(-1)).toMatch(/Abendrunde.*wurde aktualisiert\.$/));
    expect(calls('event_groups_for_events')).toHaveLength(0);
    expect(openEventGroupCarry).not.toHaveBeenCalled();
  });
});

describe('EventDialog → a new series keeps its typed times on the new clock', () => {
  it('says the typed times, and creates the series on the zone chosen', async () => {
    deviceInBerlin();
    await open(null);
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
      target: { value: 'Neu' },
    });
    fireEvent.change(screen.getByRole('combobox', { name: /^wiederholung$/i }), {
      target: { value: 'WEEKLY' },
    });
    const typed = {
      date: input(/^startdatum$/i).value,
      time: input(/^startzeit$/i).value,
    };
    await pick('New York', 'New York');
    await settle();
    await waitFor(() => expect(document.activeElement).toBe(zoneButton()));
    expect(announced).toHaveLength(1);
    expect(announced[0]).toMatch(
      /^Zeitzone der Serie: New York\. Die Uhrzeiten bleiben wie getippt und gelten jetzt auf dieser Uhr: /,
    );
    expect(input(/^startdatum \(new york\)$/i).value).toBe(typed.date);
    expect(input(/^startzeit \(new york\)$/i).value).toBe(typed.time);
    expect(screen.queryByRole('dialog', { name: 'Welcher Wochentag soll gelten?' })).toBeNull();

    save(/^anlegen$/i);
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));
    const request = (calls('create_event')[0][1] as {
      request: { start: string; recurrence: Record<string, unknown> };
    }).request;
    const expected = fieldInstant(
      { ...typed, at: null },
      { kind: 'series', tzid: 'America/New_York' },
    );
    expect(expected.kind).toBe('at');
    expect(Date.parse(request.start)).toBe(Date.parse((expected as { at: string }).at));
    expect(request.recurrence.tzid).toBe('America/New_York');
  });
});
