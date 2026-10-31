import { useEffect, useSyncExternalStore } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * The prefill has to survive a render that comes BEFORE its own update.
 *
 * Toni's log from a release build (2026-10-31, a trace since removed): the reset
 * took its first-hydrate shortcut, the prefill applied the offer's calendar,
 * and two milliseconds later the reset ran AGAIN, still finding the form
 * pristine — the prefill's update had not rendered yet. An update a store
 * pushes synchronously (`useSyncExternalStore`, a warm cache) renders at a
 * higher priority than the updates the effects queued, so that render still
 * shows the baseline while the editor's inputs have moved on. The reset then
 * queued the baseline behind the prefill, and the prefill, latched and with
 * nothing in its dependencies changed, never ran again. StrictMode in dev
 * re-runs both effects and hid it.
 *
 * So this renders WITHOUT StrictMode, and a store that the editor reads
 * pushes a change synchronously right after the editor's effects have run.
 */

vi.mock('@tauri-apps/api/core', () => ({ invoke: () => Promise.resolve([]) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-local', name: 'Kalender', read_only: false, account_id: 'local' } as unknown as Calendar,
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'local' } as unknown as Calendar,
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

/** A calendar store that pushes its changes synchronously, as a warm one does. */
const store = vi.hoisted(() => {
  let snapshot = {
    calendars: [] as unknown[],
    colorLabels: [] as unknown[],
    selectedCalendarIds: new Set<string>(),
  };
  const listeners = new Set<() => void>();
  return {
    get: () => snapshot,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    set: (next: typeof snapshot) => {
      snapshot = next;
      listeners.forEach((listener) => listener());
    },
  };
});

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => useSyncExternalStore(store.subscribe, store.get),
}));
// Every hook's answer stable across renders, as the app's are.
const stable = vi.hoisted(() => ({
  view: { showHiddenCalendarTargets: false, anchor: new Date() },
  dialogs: { openEventGroupCarry: () => {} },
  reminders: { getDefaultsFor: () => [] },
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => stable.view }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => stable.dialogs }));
// Stable, as the app's is: a new function per render would re-run the
// prefill effect on every render and hide the bug.
const announce = vi.hoisted(() => () => {});
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => stable.reminders,
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [] };
});

/** Rendered after the editor: its effect runs right after the editor's. */
function StorePushesAfterTheEditor() {
  useEffect(() => {
    const current = store.get();
    // The same calendars, the selection as a new set: every input of the
    // editor's baseline that reads it moves.
    store.set({ ...current, selectedCalendarIds: new Set(current.selectedCalendarIds) });
  }, []);
  return null;
}

describe('EventDialog prefill → calendar, a store pushing before the prefill renders', () => {
  it("keeps the offer's calendar", async () => {
    store.set({
      calendars: CALENDARS,
      colorLabels: [],
      selectedCalendarIds: new Set(['cal-local', 'cal-work']),
    });
    const { EventDialog } = await import('./EventDialog');
    render(
      <>
        <EventDialog
          isOpen
          onClose={() => {}}
          event={null}
          defaultCalendarId="cal-local"
          defaultTitle={SOURCE.title}
          prefillFrom={SOURCE}
          targetPinned={false}
        />
        <StorePushesAfterTheEditor />
      </>,
    );
    const select = screen.getByRole('combobox', { name: /kalender/i }) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('cal-work'));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(select.value).toBe('cal-work');
  });
});
