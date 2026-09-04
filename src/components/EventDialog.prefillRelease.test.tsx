import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * The prefill has to survive a RELEASE build, where React does not
 * double-invoke effects.
 *
 * `EventDialog.prefill.test.tsx` renders in StrictMode on purpose — the
 * double invocation was the original bug. But it also HIDES the opposite
 * failure: the dialog adopts a fresh baseline while the form is still
 * pristine, and its reads resolve asynchronously. If one of them lands after
 * the prefill has marked itself applied but before the form has moved away
 * from the baseline, the reset puts the baseline back and the prefill never
 * runs again — the calendar the user was shown in the quick-add is silently
 * replaced by the default one.
 *
 * So this file renders WITHOUT StrictMode, and makes the per-event read
 * resolve late, which is what a real backend does.
 */

/** Resolves `list_event_local_reminders` only when the test says so. */
let releasePrivateRead: (() => void) | null = null;
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) => {
    if (command === 'list_event_local_reminders') {
      return new Promise((resolve) => {
        releasePrivateRead = () => resolve([]);
      });
    }
    return Promise.resolve([]);
  }),
);
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

// Mutable, because the real catalog arrives a beat after the dialog opens.
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
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
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

describe('EventDialog prefill → calendar, without StrictMode', () => {
  it("keeps the earlier appointment's calendar when a read resolves late", async () => {
    const { EventDialog } = await import('./EventDialog');
    render(
      <EventDialog
        isOpen
        onClose={() => {}}
        event={null}
        defaultCalendarId="cal-local"
        defaultTitle={SOURCE.title}
        prefillFrom={SOURCE}
        targetPinned={false}
      />,
    );
    const select = screen.getByRole('combobox', {
      name: /kalender/i,
    }) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('cal-work'));

    // Now the per-event read comes back, re-deriving the baseline. The
    // prefill has already run once and will not run again — the calendar the
    // quick-add showed has to stay.
    releasePrivateRead?.();
    await waitFor(() => expect(invokeMock).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(select.value).toBe('cal-work');
  });

  it("waits for the calendar catalog before deciding the offer's calendar", async () => {
    // The catalog is fetched, so on a cold start the dialog opens before it
    // arrives. The prefill decides whether the offer's calendar can take an
    // appointment by looking it up in that catalog — with an empty one every
    // calendar reads as unknown, the offer is refused, and the editor opens on
    // the default. Deciding that once and never again is the bug: by the time
    // the catalog lands, the prefill has marked itself done.
    const previous = STORE.calendars;
    STORE.calendars = [];
    try {
      const { EventDialog } = await import('./EventDialog');
      const dialog = (
        <EventDialog
          isOpen
          onClose={() => {}}
          event={null}
          defaultCalendarId="cal-local"
          defaultTitle={SOURCE.title}
          prefillFrom={SOURCE}
          targetPinned={false}
        />
      );
      const { rerender } = render(dialog);
      // The catalog arrives.
      STORE.calendars = previous;
      rerender(dialog);
      const select = screen.getByRole('combobox', {
        name: /kalender/i,
      }) as HTMLSelectElement;
      await waitFor(() => expect(select.value).toBe('cal-work'));
    } finally {
      STORE.calendars = previous;
    }
  });
});
