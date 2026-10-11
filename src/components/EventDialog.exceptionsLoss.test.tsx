import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Saving a series that would make the provider drop occurrences the user
 * changed or deleted on their own asks first (decisions 243-253).
 *
 * The adapter knows what the save would rewrite and how many occurrences that
 * loses; without the user's consent it sends nothing and refuses with
 * `exceptions-would-be-lost: {rewrite}:{changed}:{deleted}`. The editor asks
 * with that, in a dialog over the form whose focus starts on Cancel. Yes saves
 * the same form again with `accepts_exception_loss`, on that one write only;
 * Cancel sends nothing more. And where deleted occurrences came back and could
 * not be deleted again, the save stands and the editor names their days.
 */

const { invokeMock, answers, announced, announce } = vi.hoisted(() => {
  const announced: string[] = [];
  const announce = (message: string) => {
    announced.push(message);
  };
  /** What each `update_event` answers, in order; then the event as sent. */
  const answers: Array<(event: unknown) => Promise<unknown>> = [];
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      const event = (payload as { event: unknown }).event;
      const next = answers.shift();
      return next ? next(event) : Promise.resolve(event);
    }
    if (command === 'get_series_rows') {
      return Promise.resolve({ rows: [], reach: { kind: 'complete' } });
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, answers, announced, announce };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'acc-ews' } as unknown as Calendar,
];

/** A weekly Monday series, as the editor opens it with the series scope. */
const SERIES: CalendarEvent = {
  id: 'M:series|CK',
  calendar_id: 'cal-work',
  title: 'Teamrunde',
  description: null,
  location: null,
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T10:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO;COUNT=4',
    exceptions: ['2026-11-09T09:00:00.000Z'],
    tzid: 'Europe/Berlin',
  },
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
  document.body.innerHTML = '';
  invokeMock.mockClear();
  answers.length = 0;
  announced.length = 0;
  vi.restoreAllMocks();
});

/** The refusal as the desktop host carries it. */
function refusal(detail: string) {
  return () => Promise.reject({ code: 'forbidden', message: `exceptions-would-be-lost: ${detail}` });
}

function updatesSent(): CalendarEvent[] {
  return invokeMock.mock.calls
    .filter((call) => call[0] === 'update_event')
    .map((call) => (call[1] as { event: CalendarEvent }).event);
}

async function openAndSave(onClose: () => void = () => {}) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={onClose} event={SERIES} />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
  fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
  await waitFor(() => expect(updatesSent().length).toBe(1));
}

describe('EventDialog → a save that would drop occurrences of the series', () => {
  it('asks, and on yes saves the same form again with the consent', async () => {
    answers.push(refusal('slot:1:0'));
    const onClose = vi.fn();
    await openAndSave(onClose);
    const dialog = await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
    // What the save rewrites and loses, with the title, is the question.
    expect(dialog.textContent).toMatch(/Teamrunde/);
    expect(dialog.textContent).toMatch(/Beginn und Ende der Serie neu/);
    expect(dialog.textContent).toMatch(/ein Vorkommen, das einzeln geändert wurde/);
    // The first write went out without the consent.
    expect(updatesSent()[0].accepts_exception_loss).toBeFalsy();

    fireEvent.click(within(dialog).getByRole('button', { name: /trotzdem speichern/i }));
    await waitFor(() => expect(updatesSent().length).toBe(2));
    expect(updatesSent()[1].accepts_exception_loss).toBe(true);
    // Everything else is the same save.
    expect({ ...updatesSent()[1], accepts_exception_loss: undefined }).toEqual({
      ...updatesSent()[0],
      accepts_exception_loss: undefined,
    });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('sends nothing more on cancel, and the form stays', async () => {
    answers.push(refusal('zone:2:1'));
    const onClose = vi.fn();
    await openAndSave(onClose);
    const dialog = await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
    expect(dialog.textContent).toMatch(/andere Zeitzone/);
    expect(dialog.textContent).toMatch(/2 einzeln geänderte Vorkommen und ein gelöschtes Vorkommen/);
    fireEvent.click(within(dialog).getByRole('button', { name: /^abbrechen$|^cancel$/i }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /vorkommen gehen verloren/i })).toBeNull(),
    );
    expect(updatesSent().length).toBe(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: /^speichern$|^save$/i })).toBeTruthy();
  });

  it('leaves no consent behind when the form stops the save again', async () => {
    // The form stays editable while the first save is on its way: the title
    // is cleared before the refusal comes back.
    let refuse = () => {};
    answers.push(
      () =>
        new Promise((_, reject) => {
          refuse = () =>
            reject({ code: 'forbidden', message: 'exceptions-would-be-lost: slot:1:0' });
        }),
    );
    answers.push(refusal('slot:1:0'));
    await openAndSave();
    const title = screen.getByRole('combobox', { name: /^titel$|^title$/i });
    fireEvent.change(title, { target: { value: '' } });
    refuse();
    const dialog = await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
    fireEvent.click(within(dialog).getByRole('button', { name: /trotzdem speichern/i }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /vorkommen gehen verloren/i })).toBeNull(),
    );
    // Stopped by the missing title: nothing went out.
    expect(updatesSent().length).toBe(1);

    // The title back and saved again: that save was never answered yes.
    fireEvent.change(title, { target: { value: 'Teamrunde' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent().length).toBe(2));
    expect(updatesSent()[1].accepts_exception_loss).toBeFalsy();
    await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
  });

  it('names the days of deleted occurrences that came back, and the save stands', async () => {
    answers.push((event) =>
      Promise.resolve({
        ...(event as object),
        deletions_not_restored: ['2026-11-16T09:00:00Z'],
      }),
    );
    const onClose = vi.fn();
    await openAndSave(onClose);
    const notice = await screen.findByText(/zurückgebracht/i);
    expect(notice.textContent).toMatch(/Teamrunde/);
    expect(notice.textContent).toMatch(/16\. November 2026/);
    expect(onClose).not.toHaveBeenCalled();
    // The notice's own button, after the dialog's header one.
    const closes = screen.getAllByRole('button', { name: /^schließen$|^close$/i });
    fireEvent.click(closes[closes.length - 1]);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });
});
