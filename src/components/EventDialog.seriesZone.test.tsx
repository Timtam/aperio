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
 * A timed series without a zone gets the device's zone when it is saved, as a
 * new one does: that repairs a series that already lost it, and gives one to an
 * event that becomes a series in the editor.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: CalendarEvent }).event);
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
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
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

/** Open the event, make the given change, save, and return the recurrence that went out. */
async function saveEdited(event: CalendarEvent, change?: () => void) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
  change?.();
  fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));
  await waitFor(() =>
    expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(true),
  );
  const update = invokeMock.mock.calls.find((call) => call[0] === 'update_event');
  return (update?.[1] as { event: CalendarEvent }).event.recurrence;
}

describe('EventDialog → editing a series as a whole', () => {
  it('keeps the zone and the exceptions of the series', async () => {
    const sent = await saveEdited(SERIES);
    expect(sent?.tzid).toBe('Europe/Berlin');
    expect(sent?.exceptions).toEqual(['2026-06-22T07:00:00.000Z']);
    expect(sent?.rrule).toMatch(/FREQ=WEEKLY/);
  });

  it("repairs a series that lost its zone: saving gives it the device's zone", async () => {
    deviceInBerlin();
    const lost = {
      ...SERIES,
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: ['2026-06-22T07:00:00.000Z'] },
    } as unknown as CalendarEvent;
    const sent = await saveEdited(lost);
    expect(sent?.tzid).toBe('Europe/Berlin');
    expect(sent?.exceptions).toEqual(['2026-06-22T07:00:00.000Z']);
  });

  it("gives a one-off event that becomes a series the device's zone", async () => {
    deviceInBerlin();
    const oneOff = { ...SERIES, recurrence: null } as unknown as CalendarEvent;
    const sent = await saveEdited(oneOff, () => {
      fireEvent.click(screen.getByRole('button', { name: 'weekly' }));
    });
    expect(sent).toEqual({ rrule: 'FREQ=WEEKLY', exceptions: [], tzid: 'Europe/Berlin' });
  });

  it('leaves an all-day series without a zone as it is', async () => {
    deviceInBerlin();
    const allDay = {
      ...SERIES,
      start: '2026-06-14T22:00:00.000Z',
      end: '2026-06-15T22:00:00.000Z',
      all_day: true,
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [] },
    } as unknown as CalendarEvent;
    const sent = await saveEdited(allDay);
    expect(sent?.tzid ?? null).toBeNull();
    expect(sent?.rrule).toMatch(/FREQ=WEEKLY/);
  });
});
