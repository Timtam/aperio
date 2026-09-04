import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * The same hand-off as `quickAddToEditor.e2e.test.tsx`, but in a RELEASE
 * build: no StrictMode, and the hooks the editor hangs off resolve
 * asynchronously the way the real backend makes them.
 *
 * StrictMode double-invokes passive effects, which gives the prefill a second
 * chance to land after any reset. Toni's report is from a release build, where
 * there is no second chance: if something re-derives the editor's baseline
 * while the form still looks pristine, the reset puts the default calendar
 * back and nothing ever runs the prefill again.
 *
 * So this file mocks NOTHING that carries the calendar: the real
 * `useCalendarDefaultReminders` (an async pref read per calendar) and the real
 * dialog stack are in play, and only the wire is faked.
 */

/** The calendar's default reminders, answered a beat late like a real read. */
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, args?: Record<string, unknown>) => {
    if (command === 'get_user_pref') {
      const key = String(args?.key ?? '');
      // Both calendars carry an ATTACHED default, which is what makes the
      // editor's create-time offer fire and re-derive its baseline.
      if (key.endsWith('.defaultReminders')) {
        return Promise.resolve(
          JSON.stringify([
            { kind: { minutes_before: 15 }, sound: null, attach: true },
          ]),
        );
      }
      return Promise.resolve(null);
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
  { id: 'cal-local', name: 'Kalender', read_only: false } as unknown as Calendar,
  { id: 'cal-work', name: 'Arbeit', read_only: false } as unknown as Calendar,
];

const SOURCE: CalendarEvent = {
  id: 'ev-thomas',
  calendar_id: 'cal-work',
  title: 'Thomas Meeting',
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

const STORE = {
  calendars: CALENDARS,
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local', 'cal-work']),
};
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [SOURCE] };
});

afterEach(() => {
  document.body.innerHTML = '';
});

async function handOff() {
  const { DialogStateProvider } = await import('../state/DialogState');
  const { useDialogState } = await import('../state/dialogStateContext');
  const { DialogHost } = await import('./DialogHost');

  function OpenQuickAdd() {
    const { openQuickAdd } = useDialogState();
    return (
      <button type="button" onClick={() => openQuickAdd({})}>
        open
      </button>
    );
  }

  render(
    <DialogStateProvider>
      <OpenQuickAdd />
      <DialogHost />
    </DialogStateProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'open' }));

  const title = await screen.findByRole('combobox', { name: /titel/i });
  fireEvent.change(title, { target: { value: 'thomas' } });
  fireEvent.keyDown(title, { key: 'ArrowDown' });
  fireEvent.keyDown(title, { key: 'Enter' });

  return (await screen.findByRole('combobox', {
    name: /kalender/i,
  })) as HTMLSelectElement;
}

describe('quick-add → accept an offer → editor, release build', () => {
  it("opens the editor on the offer's calendar and STAYS there", async () => {
    const picker = await handOff();
    await waitFor(() => expect(picker.value).toBe('cal-work'));
    // Every async read the editor started has to be allowed to land. Each one
    // re-derives the baseline, and a reset that fires on a form it wrongly
    // reads as pristine would silently put 'cal-local' back.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(picker.value).toBe('cal-work');
  }, 15_000);
});
