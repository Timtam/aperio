import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Calendar, CalendarEvent } from '../../api/types';

/**
 * The agenda moves its own focus through its entries and leaves the anchor on
 * the day the list starts on. The K key says the week of the day the view
 * stands on (TODO B15), so the agenda names the focused entry's day on its
 * list (`data-active-day`), and K reads it there. Paged onto a stretch with
 * nothing in it, the list has no entry to read, so the agenda says where it
 * now starts (TODO B15 step 3).
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
const VIEW_STATE = { anchor: new Date(2026, 10, 4), pageTurn: 0 };
const announce = vi.fn();
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
vi.mock('../../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
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

beforeEach(() => {
  VIEW_STATE.anchor = new Date(2026, 10, 4);
  VIEW_STATE.pageTurn = 0;
  announce.mockReset();
});

describe('a page turn in the agenda', () => {
  it('says where it now starts when the stretch has nothing to read', async () => {
    const { AgendaView } = await import('./AgendaView');
    const view = render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    list.focus();
    // A month on: past both meetings.
    VIEW_STATE.anchor = new Date(2026, 11, 4);
    VIEW_STATE.pageTurn += 1;
    view.rerender(<AgendaView />);
    expect(announce).toHaveBeenCalledWith('Ab Freitag, 4. Dezember 2026');
  });

  it('leaves it to the focused entry, which names its day', async () => {
    const { AgendaView } = await import('./AgendaView');
    const view = render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    list.focus();
    VIEW_STATE.anchor = new Date(2026, 10, 10);
    VIEW_STATE.pageTurn += 1;
    view.rerender(<AgendaView />);
    expect(announce).not.toHaveBeenCalled();
  });
});
