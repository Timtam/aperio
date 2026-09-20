import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * An invitation somebody else organizes, on a provider that takes only this
 * account's own reply and reminders (decision 77a).
 *
 * The editor shows it read-only: every field is still a stop the reading
 * cursor reaches, with the same label as in the editable editor, and nothing
 * is `disabled` — a disabled control is skipped, and the value is what the
 * user opened the dialog to hear. Answering and setting reminders stay.
 * Deleting stays too, and says that the organizer gets a decline (83b).
 */

const { invokeMock, onFile } = vi.hoisted(() => {
  const onFile: { loaded: unknown } = { loaded: null };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'get_event_by_id') return Promise.resolve(onFile.loaded);
    if (command === 'calendar_current_user_email') {
      return Promise.resolve('me@example.com');
    }
    if (command === 'get_user_pref') return Promise.resolve(null);
    return Promise.resolve([]);
  });
  return { invokeMock, onFile };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const ICLOUD = {
  id: 'cal-icloud',
  name: 'iCloud',
  read_only: false,
  account_id: 'acc-icloud',
  supports_scheduling: true,
  always_notifies_attendees: true,
  invitations_reply_only: true,
  notifier_name: 'iCloud',
} as unknown as Calendar;

/** The row a view hands the editor: a weekly series somebody else organizes. */
const INVITATION = {
  id: 'ev-inv',
  calendar_id: 'cal-icloud',
  title: 'Aperio R6 fremde',
  description: 'Bitte Unterlagen mitbringen',
  location: 'Raum 3',
  start: '2026-11-09T15:00:00.000Z',
  end: '2026-11-09T15:30:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY;COUNT=4', exceptions: [], tzid: null },
  color_label: null,
  reminders: [],
  attendees: ['Boss <boss@example.net>', 'me@example.com'],
  organizer: 'boss@example.net',
  organized_elsewhere: true,
  attendee_responses: [
    { email: 'me@example.com', name: 'Ich', status: 'needs_action' },
  ],
} as unknown as CalendarEvent;

const STORE = {
  calendars: [ICLOUD] as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-icloud']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = {
  openEventGroupCarry: () => {},
  invalidateData: () => {},
};
const REMINDERS = { getDefaultsFor: () => [] };

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => REMINDERS,
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  onFile.loaded = null;
});

async function open(event: CalendarEvent = INVITATION) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} initialScope="series" />
    </StrictMode>,
  );
  await screen.findByText(/organisiert jemand anderes|organizes this meeting/i, undefined, {
    timeout: 8000,
  });
}

const field = (label: RegExp): HTMLInputElement | HTMLTextAreaElement =>
  screen.getByLabelText(label) as HTMLInputElement | HTMLTextAreaElement;

describe('EventDialog → an invitation somebody else organizes', () => {
  it('says why, as the first stop, and never with a disabled control', async () => {
    await open();
    const hint = screen.getByText(/organisiert jemand anderes|organizes this meeting/i);
    // Tab reaches it, so the reason is read where it belongs and not only on
    // the dialog's opening announcement.
    expect(hint.getAttribute('tabindex')).toBe('0');
    expect(document.querySelectorAll('[disabled]')).toHaveLength(0);
  });

  it('shows the meeting read-only, with the editable editor’s own labels', async () => {
    await open();
    for (const [label, value] of [
      [/^titel$|^title$/i, 'Aperio R6 fremde'],
      [/^kalender$|^calendar$/i, 'iCloud'],
      [/^ort$|^location$/i, 'Raum 3'],
    ] as [RegExp, string][]) {
      const input = field(label);
      expect(input.readOnly).toBe(true);
      expect(input.value).toBe(value);
    }
    // The dates and times are read-only too, and the repeat rule is a
    // sentence rather than controls the user cannot use.
    expect(field(/^beginnt am$|^start date$|^startdatum$/i).readOnly).toBe(true);
    const repeat = field(/^wiederholung$|^repeat$/i);
    expect(repeat.readOnly).toBe(true);
    expect(repeat.value).toMatch(/jeden Montag, 4 Mal|every Monday, 4 times/i);
    // Nothing that would write the meeting.
    expect(screen.queryByRole('combobox', { name: /kalender|calendar/i })).toBeNull();
    expect(
      screen.queryByRole('checkbox', { name: /ganztägig|all day/i }),
    ).toBeNull();
    // The guests are shown, but not as an editor.
    const attendees = field(/^teilnehmer$|^attendees$/i);
    expect(attendees.readOnly).toBe(true);
    expect(attendees.value).toContain('boss@example.net');
  });

  it('keeps what an attendee may do: answer, set reminders, ask about availability', async () => {
    await open();
    expect(
      screen.getByRole('button', { name: /zusagen|accept/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /verfügbarkeit|availability/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /erinnerung hinzufügen|add reminder/i }),
    ).toBeInTheDocument();
  });

  it('saves the row the provider has, with only its reminders changed', async () => {
    // The provider's copy says something the form does not: only sending the
    // loaded row can carry it.
    onFile.loaded = { ...INVITATION, description: 'Vom Server, nicht aus dem Formular' };
    await open();
    fireEvent.click(
      screen.getByRole('button', { name: /erinnerung hinzufügen|add reminder/i }),
    );
    fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
    );
    const update = invokeMock.mock.calls.filter((call) => call[0] === 'update_event').pop();
    const sent = (update?.[1] as { event: CalendarEvent }).event;
    // The row as the provider has it: the form's values never reach the wire.
    expect(sent.id).toBe('ev-inv');
    expect(sent.title).toBe('Aperio R6 fremde');
    expect(sent.description).toBe('Vom Server, nicht aus dem Formular');
    expect(sent.attendees).toEqual(INVITATION.attendees);
    expect(sent.send_invitations).toBe(false);
    expect(sent.reminders.length).toBeGreaterThan(0);
    // Nothing was carved out of the series.
    expect(invokeMock.mock.calls.some((call) => call[0] === 'add_event_exdate')).toBe(false);
    expect(invokeMock.mock.calls.some((call) => call[0] === 'create_event')).toBe(false);
  });

  it('names the stored rule beside the controls that would rewrite it (87b)', async () => {
    // An editable event whose rule the repeat controls cannot hold: they show
    // "the last Monday" for "the last workday", and touching one would save
    // that. So the stored rule is said in words — under its OWN label, or two
    // adjacent stops would both be called "Wiederholung".
    const editable = {
      ...INVITATION,
      id: 'ev-own',
      organized_elsewhere: false,
      organizer: 'me@example.com',
      attendee_responses: [],
      recurrence: {
        rrule: 'FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1',
        exceptions: [],
        tzid: null,
      },
    } as unknown as CalendarEvent;
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={editable} initialScope="series" />
      </StrictMode>,
    );
    const stored = await screen.findByLabelText(
      /gespeicherte wiederholung|stored repeat/i,
      undefined,
      { timeout: 8000 },
    );
    expect((stored as HTMLInputElement).readOnly).toBe(true);
    expect((stored as HTMLInputElement).value).toMatch(
      /am letzten Werktag|on the last weekday/i,
    );
    // The controls are still there: this is an event the account may edit.
    expect(
      screen.queryByText(/organisiert jemand anderes|organizes this meeting/i),
    ).toBeNull();
  });

  it('asks before deleting, and says the organizer gets a decline', async () => {
    await open();
    fireEvent.click(screen.getByRole('button', { name: /^(löschen|delete)$/i }));
    await screen.findByText(/Organisator bekommt eine Absage|organizer gets a decline/i);
    fireEvent.click(
      screen.getByRole('button', { name: /löschen und absagen|delete and decline/i }),
    );
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'delete_event')).toBe(true),
    );
  });
});
