import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Accepting an offer in the quick-add has to bring the earlier appointment's
 * CALENDAR with it. It is the one field the user cannot see themselves fixing
 * — the hint said "Arbeit", so an editor that opens on the local calendar has
 * quietly disagreed with what it just showed.
 */

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  {
    id: 'cal-local',
    name: 'Kalender',
    read_only: false,
    account_id: 'local',
  } as unknown as Calendar,
  {
    id: 'cal-work',
    name: 'Arbeit',
    read_only: false,
    account_id: 'local',
  } as unknown as Calendar,
];

const SOURCE: CalendarEvent = {
  id: 'ev-thomas',
  calendar_id: 'cal-work',
  title: 'Thomas',
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

// Stable identities: the real store memoises these, and a fresh object (or a
// fresh Set) per render would re-derive `initialState` every render and spin
// the reset effect forever.
const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local', 'cal-work']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {} };
const REMINDERS = { getDefaultsFor: () => [] };

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
});

async function openWithPrefill(targetPinned: boolean, calendarId?: string) {
  const { EventDialog } = await import('./EventDialog');
  // StrictMode, because the app runs in it (src/main.tsx) and because its
  // double-invoked passive effects are the whole bug: without it the reset
  // effect runs once and every one of these passed while the real dialog was
  // dropping the prefill.
  render(
    <StrictMode>
      <EventDialog
        isOpen
        onClose={() => {}}
        event={null}
        defaultCalendarId={calendarId}
        defaultTitle={SOURCE.title}
        prefillFrom={SOURCE}
        targetPinned={targetPinned}
      />
    </StrictMode>,
  );
  return screen.getByRole('combobox', { name: /kalender/i }) as HTMLSelectElement;
}

describe('EventDialog prefill → calendar', () => {
  it("adopts the earlier appointment's calendar when nothing was pinned", async () => {
    // The quick-add passes its own untouched default as `calendarId`. That is
    // NOT a choice, and the offer's calendar has to win over it.
    const select = await openWithPrefill(false, 'cal-local');
    await waitFor(() => expect(select.value).toBe('cal-work'));
  });

  it('keeps the pinned calendar when the user picked one', async () => {
    const select = await openWithPrefill(true, 'cal-local');
    await waitFor(() => expect(select.value).toBe('cal-local'));
  });

  it('says WHY when the offer lives on a calendar it cannot use', async () => {
    // The input that explains every observation: the quick-add's hint looks
    // the calendar up by id with NO writability check, so it can say "Arbeit"
    // for a calendar `applyEventPrefill` then refuses. Refusing is right — a
    // read-only calendar rejects the write — but it used to happen in total
    // silence, so the editor simply disagreed with what it had just shown.
    const readOnlyWork = [
      CALENDARS[0],
      { ...CALENDARS[1], read_only: true } as unknown as Calendar,
    ];
    const previous = STORE.calendars;
    STORE.calendars = readOnlyWork;
    try {
      const select = await openWithPrefill(false, 'cal-local');
      await waitFor(() => expect(select.value).toBe('cal-local'));
      expect(await screen.findByText(/Arbeit/)).toBeInTheDocument();
    } finally {
      STORE.calendars = previous;
    }
  });
});
