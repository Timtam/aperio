import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { DefaultReminder } from '@aperio/shared';
import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Which of a calendar's default reminders may appear as an editable row.
 *
 * Only an ATTACHED default stands in for the event's own reminders — the core
 * drops it as soon as the event carries any. An entry that stays "only in
 * Aperio" fires ON TOP of whatever the event carries, so offering it as a row
 * would lie to the user: changing 15 to 30 would not move that reminder, it
 * would add a second one and leave the first ringing, with nothing on screen
 * or in the screen reader to show it.
 */

/** Rows the host would return for `list_event_local_reminders`. Swapped per
 *  test; everything else answers with an empty list, as before. */
const privateRows = vi.hoisted(() => ({ current: [] as unknown[] }));
/** What the host would return for `update_event` — a real event shape, since
 *  the dialog keys the private row by what came BACK from the save. */
const savedEvent = vi.hoisted(() => ({ current: null as unknown }));
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
    void payload;
    if (command === 'list_event_local_reminders') return Promise.resolve(privateRows.current);
    if (command === 'update_event' && savedEvent.current) {
      return Promise.resolve(savedEvent.current);
    }
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

// An EXTERNAL calendar: the placement choice only exists where there is
// somebody else to tell. The local case is asserted on its own below.
const CALENDARS: Calendar[] = [
  {
    id: 'cal-work',
    name: 'Arbeit',
    read_only: false,
    account_id: 'acc-icloud',
  } as unknown as Calendar,
];

/** An iCloud-shaped event: no VALARM of its own. */
const EVENT: CalendarEvent = {
  id: 'ev-1',
  calendar_id: 'cal-work',
  title: 'Zahnarzt',
  description: null,
  location: null,
  start: '2026-06-15T09:00:00.000Z',
  end: '2026-06-15T10:00:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

const ATTACHED: DefaultReminder = {
  kind: { type: 'relative', minutes_before: 60 },
  sound: null,
  attach: true,
};
const IN_APERIO: DefaultReminder = {
  kind: { type: 'relative', minutes_before: 1440 },
  sound: null,
};

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {} };
/** Swapped per test before mounting. */
let defaults: DefaultReminder[] = [];

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({
  useViewState: () => VIEW_STATE,
}));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => DIALOG_STATE,
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
// ONE object with ONE function, like the sibling prefill test. A fresh
// identity per render would make the dialog re-derive its baseline on every
// render — the editor is built to adopt a fresh baseline while the form is
// pristine, so a churning mock turns that into an endless loop and the test
// spins instead of failing. The mutable `defaults` behind it is what each test
// swaps.
const REMINDERS = { getDefaultsFor: () => defaults };
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => REMINDERS,
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  defaults = [];
  privateRows.current = [];
  savedEvent.current = null;
});

async function openEditor(calendarDefaults: DefaultReminder[]) {
  defaults = calendarDefaults;
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={EVENT} />
    </StrictMode>,
  );
  // The dialog is up once its calendar picker is. The generous window is for
  // the full suite: this renders a whole editor, and under parallel load the
  // 1s default expires while the machine is still busy elsewhere.
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

const reminderRows = () => screen.queryAllByRole('group', { name: /Erinnerung \d/i });

describe('EventDialog → the calendar defaults it shows', () => {
  it('shows an attached default as the event\'s own reminder row', async () => {
    await openEditor([ATTACHED]);
    await waitFor(() => expect(reminderRows()).toHaveLength(1));
  });

  it('never shows an "only in Aperio" default as a row', async () => {
    // It fires on top of the event's own reminders and cannot be edited from
    // here — the settings page owns it.
    await openEditor([IN_APERIO]);
    await waitFor(() =>
      expect(screen.getByRole('combobox', { name: /kalender/i })).toBeInTheDocument(),
    );
    expect(reminderRows()).toHaveLength(0);
  });

  it('shows only the attached half of a mixed list', async () => {
    await openEditor([IN_APERIO, ATTACHED]);
    await waitFor(() => expect(reminderRows()).toHaveLength(1));
  });
});

describe('EventDialog → creating an appointment', () => {
  async function openCreate(calendarDefaults: DefaultReminder[]) {
    defaults = calendarDefaults;
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog
          isOpen
          onClose={() => {}}
          event={null}
          defaultCalendarId="cal-work"
        />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i });
  }

  it("offers the calendar's attached defaults as editable rows", async () => {
    // They would be written in on save either way. Showing them is what lets
    // the user see, change or remove one BEFORE it lands on the appointment.
    await openCreate([ATTACHED]);
    await waitFor(() => expect(reminderRows()).toHaveLength(1));
    const applies = screen.getAllByRole('combobox', { name: /Gilt|Applies/i });
    expect(applies.map((el) => (el as HTMLSelectElement).value)).toEqual(['attach']);
  });

  it('never offers an "only in Aperio" default as a row', async () => {
    // That one fires beside the appointment's own reminders rather than
    // instead of them, so a row for it would lie about what editing it does.
    await openCreate([IN_APERIO]);
    await waitFor(() =>
      expect(screen.getByRole('combobox', { name: /kalender/i })).toBeInTheDocument(),
    );
    expect(reminderRows()).toHaveLength(0);
  });

  it('tells the host no reminder choice was made when nothing was touched', async () => {
    // The POSITIVE half of `use_calendar_defaults`, which nothing pinned: only
    // this flag lets the host write a calendar's attached default into the new
    // appointment. Silently sending `false` would leave the alarm off the
    // provider — the appointment saves, the editor looks right, and only a
    // phone that stays quiet would ever say otherwise.
    savedEvent.current = { ...EVENT, reminders: [] };
    await openCreate([IN_APERIO]);
    await waitFor(() =>
      expect(screen.getByRole('combobox', { name: /kalender/i })).toBeInTheDocument(),
    );

    const title = screen.getByRole('combobox', { name: /^titel$|^title$/i });
    fireEvent.change(title, { target: { value: 'Zahnarzt' } });
    screen.getByRole('button', { name: /^anlegen$|^create$/i }).click();
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'create_event')).toBe(true),
    );
    const create = invokeMock.mock.calls.find((call) => call[0] === 'create_event');
    const payload = (
      create?.[1] as { request: { reminders: unknown[]; use_calendar_defaults?: boolean } }
    ).request;
    expect(payload.reminders).toEqual([]);
    expect(payload.use_calendar_defaults).toBe(true);
  });

  it('sends nothing when the user removes what was offered', async () => {
    savedEvent.current = { ...EVENT, reminders: [] };
    await openCreate([ATTACHED]);
    await waitFor(() => expect(reminderRows()).toHaveLength(1));
    screen.getByRole('button', { name: /Erinnerung 1 entfernen|Remove reminder 1/i }).click();
    await waitFor(() => expect(reminderRows()).toHaveLength(0));

    // A title is required, so the save would be refused without one.
    const title = screen.getByRole('combobox', { name: /^titel$|^title$/i });
    fireEvent.change(title, { target: { value: 'Zahnarzt' } });
    screen.getByRole('button', { name: /^anlegen$|^create$/i }).click();
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'create_event')).toBe(true),
    );
    const create = invokeMock.mock.calls.find((call) => call[0] === 'create_event');
    const payload = (create?.[1] as { request: { reminders: unknown[]; use_calendar_defaults?: boolean } })
      .request;
    expect(payload.reminders).toEqual([]);
    // The removal is a decision, so the host must not put them back.
    expect(payload.use_calendar_defaults).toBe(false);
  });
});

describe('EventDialog → reminders Aperio keeps for this event', () => {
  it('shows a private reminder beside the ones on the appointment', async () => {
    // The event carries one alarm the provider stores; Aperio keeps a second
    // one for this appointment alone. Both belong in the list — the row says
    // which is which.
    privateRows.current = [
      {
        calendar_id: 'cal-work',
        event_id: 'ev-1',
        reminders: [{ kind: { type: 'relative', minutes_before: 1440 }, sound: null }],
        title: 'Zahnarzt',
        starts_at: '2026-06-15T09:00:00.000Z',
        updated_at: '2026-06-01T10:00:00.000Z',
      },
    ];
    const withOwn = {
      ...EVENT,
      reminders: [{ kind: { type: 'relative', minutes_before: 15 }, sound: null }],
    } as unknown as CalendarEvent;
    const { EventDialog } = await import('./EventDialog');
    render(
      <StrictMode>
        <EventDialog isOpen onClose={() => {}} event={withOwn} />
      </StrictMode>,
    );
    await screen.findByRole('combobox', { name: /kalender/i });
    await waitFor(() => expect(reminderRows()).toHaveLength(2));
    // Each row says where it applies — the choice only exists for a calendar
    // somebody else can read, which this one is.
    const applies = screen.getAllByRole('combobox', { name: /Gilt|Applies/i });
    expect(applies).toHaveLength(2);
    expect(applies.map((el) => (el as HTMLSelectElement).value)).toEqual([
      'attach',
      'local',
    ]);
  });

  it('keeps them when an unrelated edit is saved without touching reminders', async () => {
    // The event carries no reminder of its own and the calendar has an
    // ATTACHED default, so the dialog is in its "keep as default" state — the
    // gate that governs the WIRE list. It must not speak for the private rows:
    // saving after changing only the location once deleted them here and, over
    // sync, on every other device.
    const stored = [
      { kind: { type: 'relative', minutes_before: 1440 }, sound: null },
    ];
    privateRows.current = [
      {
        calendar_id: 'cal-work',
        event_id: 'ev-1',
        reminders: stored,
        title: 'Zahnarzt',
        starts_at: '2026-06-15T09:00:00.000Z',
        updated_at: '2026-06-01T10:00:00.000Z',
      },
    ];
    savedEvent.current = { ...EVENT, reminders: [] };
    await openEditor([ATTACHED]);
    // Two rows: the calendar's attached default, and the private one.
    await waitFor(() => expect(reminderRows()).toHaveLength(2));

    screen.getByRole('button', { name: /speichern|save/i }).click();
    // The save is done once the event itself went out; `act` is deliberately
    // not used here — it would keep flushing this dialog's own re-renders.
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
    );

    const wrote = invokeMock.mock.calls.filter(
      (call) => call[0] === 'set_event_local_reminders',
    );
    // Either nothing was written, or exactly what was stored — never an
    // empty list, which is what deleted them.
    for (const call of wrote) {
      const payload = call[1] as { reminders: unknown } | undefined;
      expect(payload?.reminders).toEqual(stored);
    }
  });

  it('offers no placement choice on a calendar only Aperio reads', async () => {
    const previous = STORE.calendars;
    STORE.calendars = [
      { ...CALENDARS[0], account_id: 'local' } as unknown as Calendar,
    ];
    try {
      await openEditor([ATTACHED]);
      await waitFor(() => expect(reminderRows()).toHaveLength(1));
      expect(screen.queryAllByRole('combobox', { name: /Gilt|Applies/i })).toHaveLength(0);
    } finally {
      STORE.calendars = previous;
    }
  });
});
