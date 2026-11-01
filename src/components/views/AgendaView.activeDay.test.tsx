import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { Calendar, CalendarEvent } from '../../api/types';

/**
 * The agenda moves its own focus through its entries and leaves the anchor on
 * the day the list starts on. The K key says the week of the day the view
 * stands on (TODO B15), so the agenda names the focused entry's day on its
 * list (`data-active-day`), and K reads it there.
 */

vi.mock('@tauri-apps/api/core', () => ({ invoke: () => Promise.resolve([]) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDAR = { id: 'work', name: 'Arbeit', read_only: false } as unknown as Calendar;

function meeting(id: string, title: string, day: number): CalendarEvent {
  return {
    id,
    calendar_id: CALENDAR.id,
    title,
    description: null,
    location: null,
    start: new Date(2026, 10, day, 10, 0).toISOString(),
    end: new Date(2026, 10, day, 11, 0).toISOString(),
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    attendees: [],
    organizer: null,
    organized_elsewhere: false,
  } as unknown as CalendarEvent;
}

// Wednesday, 4 November, in week 45, and Wednesday, 18 November, in week 47.
// Stable identities: a fresh array per render would re-derive the rows forever.
const EVENTS = [meeting('a', 'Planung', 4), meeting('b', 'Review', 18)];
const CALENDAR_BY_ID = new Map([[CALENDAR.id, CALENDAR]]);
const GROUPS: unknown[] = [];
const STORE = {
  calendars: [CALENDAR],
  colorLabels: [],
  selectedCalendarIds: new Set([CALENDAR.id]),
};
const VIEW_STATE = { anchor: new Date(2026, 10, 4) };
const DIALOG_STATE = {
  openEventDialog: () => {},
  openMoveCopy: () => {},
  invalidateData: () => {},
  dataVersion: 0,
  stepWriteWait:
    () =>
    async <T,>(_inSteps: boolean, write: () => Promise<T>): Promise<T> =>
      write(),
};
const MENU = { openForEvent: () => {} };

vi.mock('../../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../../state/useEvents', () => ({
  useEvents: () => ({ events: EVENTS, calendarById: CALENDAR_BY_ID, loading: false }),
}));
vi.mock('../../state/useEventGroups', () => ({
  useEventGroups: (events: CalendarEvent[]) => ({ groups: GROUPS, events }),
}));
vi.mock('../../state/useChipContextMenu', () => ({ useChipContextMenu: () => MENU }));
vi.mock('../../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../DayCheckInButton', () => ({ DayCheckInButton: () => null }));

describe('the agenda names the day of its focused entry', () => {
  it('for the K key, moving with the focus', async () => {
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Review/);
    expect(list).toHaveAttribute('data-active-day', '2026-11-04');
    fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(list).toHaveAttribute('data-active-day', '2026-11-18');
  });
});
