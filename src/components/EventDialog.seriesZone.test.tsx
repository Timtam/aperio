import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Editing a series as a whole keeps the zone it was written in.
 *
 * The dialog used to rebuild the recurrence from the form as
 * `{rrule, exceptions}`, which dropped `tzid`. Every writer stores what it is
 * handed, so the series came back without its zone — in the local store and on
 * CalDAV, Google, Graph and EWS alike — and slid an hour at the next clock
 * change, in the views, in its reminders and in every other client.
 *
 * An event that becomes a series in the editor gets the device's zone, as a new
 * series does. A series that already recurs without a zone is left as it is: it
 * may be meant to run in UTC.
 *
 * A row of a series opened with the whole-series scope holds that occurrence's
 * fields. It saves onto the series, which it loads: the series keeps its start,
 * its rule and its exceptions unless the edit changed them.
 */

const { invokeMock, onFile, announced, announce } = vi.hoisted(() => {
  /** Everything the dialog hands the screen reader's live region, in order. */
  const announced: string[] = [];
  const announce = (message: string) => {
    announced.push(message);
  };
  /** What `get_event_by_id` answers: the series a row of a series belongs to. */
  const onFile: { series: unknown; updated: unknown } = { series: null, updated: null };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      // What the provider answers: the event as sent, unless a test says otherwise.
      return Promise.resolve(onFile.updated ?? (payload as { event: unknown }).event);
    }
    if (command === 'get_event_by_id') {
      return Promise.resolve(onFile.series);
    }
    // A string command: the RSVP block asks whose calendar this is.
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
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'acc-icloud' } as unknown as Calendar,
];

/** A weekly Monday 09:00 series written in Berlin summer time, one Monday excluded. */
const SERIES: CalendarEvent = {
  id: 'ev-series',
  calendar_id: 'cal-work',
  title: 'Teamrunde',
  description: null,
  location: null,
  start: '2026-06-15T07:00:00.000Z',
  end: '2026-06-15T08:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO',
    exceptions: ['2026-06-22T07:00:00.000Z'],
    tzid: 'Europe/Berlin',
  },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

/** A later Monday of SERIES, as the views expand it. */
const JULY_OCCURRENCE = {
  ...SERIES,
  id: 'ev-series@2026-07-06T07:00:00.000Z',
  series_id: 'ev-series',
  occurrence_start: '2026-07-06T07:00:00.000Z',
  start: '2026-07-06T07:00:00.000Z',
  end: '2026-07-06T08:00:00.000Z',
} as unknown as CalendarEvent;

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {} };
// One object with one function, like the sibling dialog tests: a fresh
// identity per render would make the editor re-derive its baseline forever.
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
// The rule picker, reduced to one button: the one-off case turns an event into
// a weekly series without driving the real picker's controls. Until it is
// pressed the form keeps the rule the event was opened with.
vi.mock('./RecurrenceSelector', () => ({
  RecurrenceSelector: ({ onChange }: { onChange: (rrule: string | null) => void }) => (
    <button type="button" onClick={() => onChange('FREQ=WEEKLY')}>
      weekly
    </button>
  ),
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  onFile.series = null;
  onFile.updated = null;
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

/** Open the event, make the given change, save, and return the event that went out. */
async function saveEditedEvent(
  event: CalendarEvent,
  change?: () => void,
  initialScope?: 'series',
): Promise<CalendarEvent> {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} initialScope={initialScope} />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
  change?.();
  fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
  await waitFor(() =>
    expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
  );
  const update = invokeMock.mock.calls.find((call) => call[0] === 'update_event');
  return (update?.[1] as { event: CalendarEvent }).event;
}

/** Like {@link saveEditedEvent}, returning only the recurrence that went out. */
async function saveEdited(event: CalendarEvent, change?: () => void, initialScope?: 'series') {
  return (await saveEditedEvent(event, change, initialScope)).recurrence;
}

describe('EventDialog → editing a series as a whole', () => {
  it('keeps the zone and the exceptions of the series', async () => {
    // A zone other than the device's, so replacing it would show.
    deviceInBerlin();
    const inNewYork = {
      ...SERIES,
      recurrence: { ...SERIES.recurrence, tzid: 'America/New_York' },
    } as unknown as CalendarEvent;
    const sent = await saveEdited(inNewYork);
    expect(sent?.tzid).toBe('America/New_York');
    expect(sent?.exceptions).toEqual(['2026-06-22T07:00:00.000Z']);
    expect(sent?.rrule).toMatch(/FREQ=WEEKLY/);
  });

  it('leaves a series without a zone as it is', async () => {
    // It may be a UTC series (Google's Etc/UTC arrives without a zone); giving
    // it the device's zone would move a late-evening rule to another weekday.
    deviceInBerlin();
    const utc = {
      ...SERIES,
      start: '2026-06-15T23:30:00.000Z',
      end: '2026-06-16T00:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: ['2026-06-22T23:30:00.000Z'] },
    } as unknown as CalendarEvent;
    const sent = await saveEdited(utc);
    expect(sent?.tzid ?? null).toBeNull();
    expect(sent?.exceptions).toEqual(['2026-06-22T23:30:00.000Z']);
  });

  it("gives a one-off event that becomes a series the device's zone", async () => {
    deviceInBerlin();
    const oneOff = { ...SERIES, recurrence: null } as unknown as CalendarEvent;
    const sent = await saveEdited(oneOff, () => {
      fireEvent.click(screen.getByRole('button', { name: 'weekly' }));
    });
    expect(sent).toEqual({ rrule: 'FREQ=WEEKLY', exceptions: [], tzid: 'Europe/Berlin' });
  });

  it('gives an all-day event that becomes a series no zone', async () => {
    // Fails if the editor hands the helper the wrong all-day state.
    deviceInBerlin();
    const allDayOneOff = {
      ...SERIES,
      start: '2026-06-14T22:00:00.000Z',
      end: '2026-06-15T22:00:00.000Z',
      all_day: true,
      recurrence: null,
    } as unknown as CalendarEvent;
    const sent = await saveEdited(allDayOneOff, () => {
      fireEvent.click(screen.getByRole('button', { name: 'weekly' }));
    });
    expect(sent).toEqual({ rrule: 'FREQ=WEEKLY', exceptions: [] });
  });

  it('does not treat an override opened as the whole series as a new series', async () => {
    // A provider override (CalDAV, EWS) carries no rule of its own. Saved as
    // the whole series it lands on the series, which may recur without a zone
    // on purpose, so nothing is stamped, and the series keeps its exceptions.
    deviceInBerlin();
    onFile.series = {
      ...SERIES,
      start: '2026-06-15T23:30:00.000Z',
      end: '2026-06-16T00:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: ['2026-06-29T23:30:00.000Z'] },
    };
    const override = {
      ...SERIES,
      id: 'ev-series::rid::2026-06-22T23:30:00Z',
      start: '2026-06-22T23:30:00.000Z',
      end: '2026-06-23T00:30:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    const sent = await saveEdited(
      override,
      () => {
        fireEvent.click(screen.getByRole('button', { name: 'weekly' }));
      },
      'series',
    );
    expect(sent?.tzid ?? null).toBeNull();
    expect(sent?.rrule).toBe('FREQ=WEEKLY');
    expect(sent?.exceptions).toEqual(['2026-06-29T23:30:00.000Z']);
  });
});

describe('EventDialog → a row of a series saved as the whole series', () => {
  it('keeps the series start when the dates are untouched', async () => {
    // The form holds the July occurrence; writing it as it is moved the series
    // start to July and the June Mondays disappeared.
    deviceInBerlin();
    onFile.series = SERIES;
    const sent = await saveEditedEvent(JULY_OCCURRENCE, undefined, 'series');
    expect(sent.id).toBe('ev-series');
    expect(sent.start).toBe(SERIES.start);
    expect(sent.end).toBe(SERIES.end);
    expect(sent.recurrence).toEqual(SERIES.recurrence);
  });

  it('gives the whole series a new time and takes its exceptions along', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    const sent = await saveEditedEvent(
      JULY_OCCURRENCE,
      () => {
        fireEvent.change(screen.getByLabelText(/startzeit|start time/i), {
          target: { value: '10:30' },
        });
      },
      'series',
    );
    const start = new Date(sent.start);
    // The series' own day, at the new time.
    expect([start.getHours(), start.getMinutes()]).toEqual([10, 30]);
    expect(start.toDateString()).toBe(new Date(SERIES.start).toDateString());
    // The excluded Monday moved with it, or it would come back at 10:30.
    const moved = Date.parse(sent.start) - Date.parse(SERIES.start);
    expect(moved).not.toBe(0);
    expect(sent.recurrence?.exceptions).toEqual([
      new Date(Date.parse(SERIES.recurrence!.exceptions[0]) + moved).toISOString(),
    ]);
  });

  it('keeps the series rule when an override is saved unchanged', async () => {
    // An override carries no rule, and the form had none to send: the series
    // lost its rule.
    deviceInBerlin();
    onFile.series = SERIES;
    const override = {
      ...SERIES,
      id: 'ev-series::rid::2026-07-06T07:00:00Z',
      start: '2026-07-06T07:30:00.000Z',
      end: '2026-07-06T08:30:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    const sent = await saveEditedEvent(override, undefined, 'series');
    expect(sent.recurrence).toEqual(SERIES.recurrence);
    expect(sent.start).toBe(SERIES.start);
  });
});

describe('EventDialog → one changed occurrence saved in place', () => {
  it('keys the colour by the row that came back', async () => {
    // Exchange will not move an exception past a neighbouring occurrence and
    // detaches it as a single with an id of its own. The colour went to the
    // override's id, which names nothing afterwards.
    deviceInBerlin();
    const override = {
      ...SERIES,
      id: 'ev-series::rid::2026-07-06T07:00:00Z',
      start: '2026-07-06T07:30:00.000Z',
      end: '2026-07-06T08:30:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    onFile.updated = { ...override, id: 'S:NEW-ID|NCK' };
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={override} initialScope="occurrence" />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
    fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'set_event_color')).toBe(true),
    );

    const update = invokeMock.mock.calls.find((call) => call[0] === 'update_event');
    expect((update?.[1] as { event: CalendarEvent }).event.id).toBe(override.id);
    const colours = invokeMock.mock.calls
      .filter((call) => call[0] === 'set_event_color')
      .map((call) => call[1] as { eventId: string; colorLabelId: string | null });
    expect(colours[0].eventId).toBe('S:NEW-ID|NCK');
    expect(colours).toContainEqual(
      expect.objectContaining({ eventId: override.id, colorLabelId: null }),
    );
  });
});

describe('EventDialog → the scope chosen up front', () => {
  it('is named by the title and by a read-only field Tab reaches', async () => {
    // The title is read out when the dialog opens. The line in the form was
    // a paragraph, which Tab never reached.
    deviceInBerlin();
    onFile.series = SERIES;
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={JULY_OCCURRENCE} initialScope="occurrence" />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
    expect(
      screen.getByRole('dialog', { name: /^(nur diesen termin bearbeiten|edit this occurrence only)$/i }),
    ).toBeTruthy();
    const field = screen.getByRole('textbox', { name: /^(anwenden auf|apply to)$/i }) as HTMLInputElement;
    expect(field.readOnly).toBe(true);
    expect(field.value).toMatch(/^(nur diesen termin|this occurrence only)$/i);
    expect(field.tabIndex).toBe(0);
  });

  it('names the whole series, which opens the series itself', async () => {
    deviceInBerlin();
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={SERIES} initialScope="series" />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
    expect(
      screen.getByRole('dialog', { name: /^(ganze serie bearbeiten|edit the whole series)$/i }),
    ).toBeTruthy();
    const field = screen.getByRole('textbox', { name: /^(anwenden auf|apply to)$/i }) as HTMLInputElement;
    expect(field.readOnly).toBe(true);
    expect(field.value).toMatch(/^(ganze serie|whole series)$/i);
  });

  it('keeps the plain title for an event that is no occurrence', async () => {
    deviceInBerlin();
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={SERIES} />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
    expect(screen.getByRole('dialog', { name: /^(termin bearbeiten|edit event)$/i })).toBeTruthy();
    expect(screen.queryByRole('textbox', { name: /^(anwenden auf|apply to)$/i })).toBeNull();
  });
});

describe('EventDialog → who may notify the attendees', () => {
  // Only the organizer notifies anyone (decision 70a).
  const meeting = (organizedElsewhere: boolean) =>
    ({
      ...SERIES,
      recurrence: null,
      attendees: ['bob@example.com'],
      organizer: 'boss@example.com',
      organized_elsewhere: organizedElsewhere,
    }) as unknown as CalendarEvent;
  const notifyToggle = () =>
    screen.queryByRole('checkbox', { name: /teilnehmer benachrichtigen|notify attendees/i });

  async function open(event: CalendarEvent) {
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={event} />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
  }

  it('offers the notify toggle for a meeting the account organizes', async () => {
    deviceInBerlin();
    const cal = CALENDARS[0] as { supports_scheduling?: boolean };
    cal.supports_scheduling = true;
    try {
      await open(meeting(false));
      expect(notifyToggle()).not.toBeNull();
    } finally {
      delete cal.supports_scheduling;
    }
  });

  it('offers no notify toggle for a meeting someone else organizes', async () => {
    deviceInBerlin();
    const cal = CALENDARS[0] as { supports_scheduling?: boolean };
    cal.supports_scheduling = true;
    try {
      await open(meeting(true));
      expect(notifyToggle()).toBeNull();
      fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
      await waitFor(() =>
        expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
      );
      const update = invokeMock.mock.calls.filter((call) => call[0] === 'update_event').pop();
      expect((update?.[1] as { event: CalendarEvent }).event.send_invitations).toBe(false);
    } finally {
      delete cal.supports_scheduling;
    }
  });

  /** A calendar whose provider mails the attendees about every change and
   *  about a deletion (iCloud; decisions 76a, 80a). */
  function alwaysNotifying(): () => void {
    const cal = CALENDARS[0] as {
      supports_scheduling?: boolean;
      always_notifies_attendees?: boolean;
      notifier_name?: string;
    };
    cal.supports_scheduling = true;
    cal.always_notifies_attendees = true;
    cal.notifier_name = 'iCloud';
    return () => {
      delete cal.supports_scheduling;
      delete cal.always_notifies_attendees;
      delete cal.notifier_name;
    };
  }
  const alwaysSentence = /iCloud informiert die Teilnehmer über jede Änderung|iCloud informs the attendees of every change/i;

  it('says who informs the attendees instead of offering a choice iCloud would not keep', async () => {
    deviceInBerlin();
    const restore = alwaysNotifying();
    try {
      await open(meeting(false));
      expect(notifyToggle()).toBeNull();
      const note = screen.getByText(alwaysSentence);
      // Tab reaches it, and it is read by its text.
      expect(note.getAttribute('tabindex')).toBe('0');
      expect(note.getAttribute('aria-label')).toMatch(alwaysSentence);
      fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
      await waitFor(() =>
        expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
      );
      const update = invokeMock.mock.calls.filter((call) => call[0] === 'update_event').pop();
      expect((update?.[1] as { event: CalendarEvent }).event.send_invitations).toBe(true);
    } finally {
      restore();
    }
  });

  /**
   * Decision 98, from live round 6: the calendar says iCloud informs the
   * attendees; this event's own resource says the server may not send for it
   * (RFC 6638 `SCHEDULE-AGENT`). The event wins, and the editor says nobody
   * will hear of the change instead of promising a mail.
   */
  it('says nobody is told where the event keeps the server out (98)', async () => {
    deviceInBerlin();
    const restore = alwaysNotifying();
    try {
      await open({ ...meeting(false), scheduling_silenced: true } as CalendarEvent);
      // Still no switch: asking to notify would ask for what will not happen.
      expect(notifyToggle()).toBeNull();
      expect(screen.queryByText(alwaysSentence)).toBeNull();
      const silent = /nicht über iCloud verschickt|not sent through iCloud/i;
      const note = screen.getByText(silent);
      // The same tab stop as the sentence it replaces, read by its own text.
      expect(note.getAttribute('tabindex')).toBe('0');
      expect(note.getAttribute('aria-label')).toMatch(silent);
    } finally {
      restore();
    }
  });

  it('deletes such a meeting after a confirmation that says so, without a silent choice', async () => {
    deviceInBerlin();
    const restore = alwaysNotifying();
    try {
      await open(meeting(false));
      fireEvent.click(screen.getByRole('button', { name: /^(löschen|delete)$/i }));
      await screen.findByText(/iCloud informiert die Teilnehmer über die Absage|iCloud informs the attendees of the cancellation/i);
      expect(
        screen.queryByRole('button', { name: /ohne benachrichtigung entfernen|remove without notifying/i }),
      ).toBeNull();
      fireEvent.click(
        screen.getByRole('button', { name: /absagen & teilnehmer benachrichtigen|cancel & notify attendees/i }),
      );
      await waitFor(() =>
        expect(invokeMock.mock.calls.some((call) => call[0] === 'delete_event')).toBe(true),
      );
      const del = invokeMock.mock.calls.filter((call) => call[0] === 'delete_event').pop();
      expect((del?.[1] as { sendCancellations: boolean | null }).sendCancellations).toBe(true);
    } finally {
      restore();
    }
  });

  it('asks about someone else\'s meeting not at all on delete (70a)', async () => {
    deviceInBerlin();
    const restore = alwaysNotifying();
    try {
      await open(meeting(true));
      fireEvent.click(screen.getByRole('button', { name: /^(löschen|delete)$/i }));
      await waitFor(() =>
        expect(invokeMock.mock.calls.some((call) => call[0] === 'delete_event')).toBe(true),
      );
      expect(
        screen.queryByRole('button', { name: /absagen & teilnehmer benachrichtigen|cancel & notify attendees/i }),
      ).toBeNull();
    } finally {
      restore();
    }
  });

  // The sentence is said once, as part of what the user just did: with
  // "X added", so neither announcement cuts the other off, or after another
  // calendar is chosen. On open it is simply there.
  it('says the sentence with the first guest added, in the same announcement', async () => {
    deviceInBerlin();
    const restore = alwaysNotifying();
    try {
      await open({ ...SERIES, recurrence: null } as unknown as CalendarEvent);
      expect(announced.some((m) => alwaysSentence.test(m))).toBe(false);
      const input = document.querySelector<HTMLInputElement>('.attendee-picker__input')!;
      fireEvent.change(input, { target: { value: 'bob@example.com' } });
      fireEvent.keyDown(input, { key: 'Enter' });
      await screen.findByText(alwaysSentence);
      const said = announced.filter((m) => alwaysSentence.test(m));
      expect(said).toHaveLength(1);
      expect(said[0]).toMatch(/^bob@example\.com (hinzugefügt|added)\. /i);
      // A second guest changes nothing about who informs them.
      fireEvent.change(input, { target: { value: 'carol@example.com' } });
      fireEvent.keyDown(input, { key: 'Enter' });
      await screen.findByText('carol@example.com');
      expect(announced.filter((m) => alwaysSentence.test(m))).toHaveLength(1);
    } finally {
      restore();
    }
  });

  it('says nothing on open, and the sentence when a calendar that mails is chosen', async () => {
    deviceInBerlin();
    const icloud = {
      id: 'cal-icloud',
      name: 'iCloud',
      read_only: false,
      account_id: 'acc-icloud',
      supports_scheduling: true,
      always_notifies_attendees: true,
      notifier_name: 'iCloud',
    } as unknown as Calendar;
    CALENDARS.push(icloud);
    STORE.selectedCalendarIds.add('cal-icloud');
    try {
      await open(meeting(false));
      expect(announced.some((m) => alwaysSentence.test(m))).toBe(false);
      fireEvent.change(screen.getByRole('combobox', { name: /kalender|calendar/i }), {
        target: { value: 'cal-icloud' },
      });
      await screen.findByText(alwaysSentence);
      expect(announced.filter((m) => alwaysSentence.test(m))).toHaveLength(1);
    } finally {
      CALENDARS.pop();
      STORE.selectedCalendarIds.delete('cal-icloud');
    }
  });

  it('keeps the notify toggle when the last attendee is removed', async () => {
    // The one removed may still get a cancellation (decision 74a).
    deviceInBerlin();
    const cal = CALENDARS[0] as { supports_scheduling?: boolean };
    cal.supports_scheduling = true;
    try {
      await open(meeting(false));
      fireEvent.click(screen.getByRole('button', { name: /bob@example\.com.*(entfernen|remove)/i }));
      await waitFor(() => expect(screen.queryByRole('button', { name: /bob@example\.com/i })).toBeNull());
      expect(notifyToggle()).not.toBeNull();
      fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
      await waitFor(() =>
        expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
      );
      const update = invokeMock.mock.calls.filter((call) => call[0] === 'update_event').pop();
      const sent = (update?.[1] as { event: CalendarEvent }).event;
      expect(sent.attendees).toEqual([]);
      expect(sent.send_invitations).toBe(true);
    } finally {
      delete cal.supports_scheduling;
    }
  });
});
