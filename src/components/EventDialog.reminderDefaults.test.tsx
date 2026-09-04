import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

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

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
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
    account_id: 'local',
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
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => ({ getDefaultsFor: () => defaults }),
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
});

async function openEditor(calendarDefaults: DefaultReminder[]) {
  defaults = calendarDefaults;
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={EVENT} />
    </StrictMode>,
  );
  // The dialog is up once its calendar picker is.
  await screen.findByRole('combobox', { name: /kalender/i });
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
