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

function meeting(id: string, title: string, day: number, hour = 10): CalendarEvent {
  return {
    id,
    calendar_id: CALENDAR.id,
    title,
    description: null,
    location: null,
    start: new Date(2026, 10, day, hour, 0).toISOString(),
    end: new Date(2026, 10, day, hour + 1, 0).toISOString(),
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    attendees: [],
    organizer: null,
    organized_elsewhere: false,
  } as unknown as CalendarEvent;
}

// Wednesday 4 and Thursday 5 November, in week 45, and Wednesday 18, in 47.
// Stable identities: a fresh array per render would re-derive the rows forever.
const EVENTS = [
  meeting('a', 'Planung', 4),
  meeting('c', 'Retro', 5),
  meeting('b', 'Review', 18),
  meeting('d', 'Ausblick', 18, 15),
];
const CALENDAR_BY_ID = new Map([[CALENDAR.id, CALENDAR]]);
const GROUPS: unknown[] = [];
const STORE = {
  calendars: [CALENDAR],
  colorLabels: [],
  selectedCalendarIds: new Set([CALENDAR.id]),
};
const VIEW_STATE = {
  anchor: new Date(2026, 10, 4),
  pageTurn: 0,
  weekStartsOn: 1,
  weekNumbers: true,
};
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
/** What the calendar holds now; a test swaps in another stable array. */
const SHOWN = { events: EVENTS };
vi.mock('../../state/useEvents', () => ({
  useEvents: () => ({ events: SHOWN.events, calendarById: CALENDAR_BY_ID, loading: false }),
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
    expect(list).toHaveAttribute('data-active-day', '2026-11-05');
    fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(list).toHaveAttribute('data-active-day', '2026-11-18');
  });
});

beforeEach(() => {
  VIEW_STATE.anchor = new Date(2026, 10, 4);
  VIEW_STATE.pageTurn = 0;
  VIEW_STATE.weekNumbers = true;
  SHOWN.events = EVENTS;
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

describe('the week in the agenda', () => {
  function activeName(list: HTMLElement): string {
    const id = list.getAttribute('aria-activedescendant');
    return (id && document.getElementById(id)?.getAttribute('aria-label')) || '';
  }

  it('the entry a step lands on names its week when the step crossed into another', async () => {
    const { AgendaView } = await import('./AgendaView');
    render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Review/);
    list.focus();
    fireEvent.keyDown(list, { key: 'ArrowDown' });
    // Thursday, in the same week: no week.
    expect(activeName(list)).toMatch(/^Donnerstag, 5\. November 2026, Retro/);
    fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(activeName(list)).toMatch(/^Mittwoch, 18\. November 2026, Kalenderwoche 47, Review/);
    // The next meeting that day is a step within the week.
    fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(activeName(list)).toMatch(/^Mittwoch, 18\. November 2026, Ausblick/);
    fireEvent.keyDown(list, { key: 'ArrowUp' });
    // Back, too.
    fireEvent.keyDown(list, { key: 'ArrowUp' });
    expect(activeName(list)).toMatch(/^Donnerstag, 5\. November 2026, Kalenderwoche 45, Retro/);
  });

  it('the separators show the week on the first day each week reaches', async () => {
    const { AgendaView } = await import('./AgendaView');
    const { container } = render(<AgendaView />);
    await screen.findByText(/Review/);
    const separators = [...container.querySelectorAll('.agenda-list__day')].map((li) => li.textContent);
    expect(separators).toEqual([
      'Mittwoch, 4. November 2026 · KW 45',
      'Donnerstag, 5. November 2026',
      'Mittwoch, 18. November 2026 · KW 47',
    ]);
  });

  it('says and shows no week with weeks switched off', async () => {
    VIEW_STATE.weekNumbers = false;
    const { AgendaView } = await import('./AgendaView');
    const { container } = render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Review/);
    list.focus();
    fireEvent.keyDown(list, { key: 'End' });
    expect(activeName(list)).toMatch(/^Mittwoch, 18\. November 2026, Ausblick/);
    expect(container.textContent).not.toContain('KW');
  });
});

describe('the week in the agenda when the list changes under the focus', () => {
  function activeName(list: HTMLElement): string {
    const id = list.getAttribute('aria-activedescendant');
    return (id && document.getElementById(id)?.getAttribute('aria-label')) || '';
  }
  /** On Ausblick, the last entry, with its week already said. */
  async function onTheLastEntry() {
    const { AgendaView } = await import('./AgendaView');
    const view = render(<AgendaView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Ausblick/);
    list.focus();
    fireEvent.keyDown(list, { key: 'End' });
    fireEvent.keyDown(list, { key: 'ArrowUp' });
    fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(activeName(list)).toMatch(/^Mittwoch, 18\. November 2026, Ausblick/);
    return { list, rerender: () => view.rerender(<AgendaView />) };
  }

  it('the entry that takes the place of a deleted one names no week it did not cross', async () => {
    const { list, rerender } = await onTheLastEntry();
    // Ausblick is deleted; Review, on the same day, is now the last entry.
    SHOWN.events = EVENTS.slice(0, 3);
    rerender();
    expect(activeName(list)).toMatch(/^Mittwoch, 18\. November 2026, Review/);
  });

  it('a list that empties for a moment is no step: the next entry is compared with the last', async () => {
    const { list, rerender } = await onTheLastEntry();
    // A reload: nothing while it reads, then the 18th alone. The focus starts
    // again at the top, on Review, in the week Ausblick was in.
    SHOWN.events = [];
    rerender();
    SHOWN.events = EVENTS.slice(2);
    rerender();
    expect(activeName(list)).toMatch(/^Mittwoch, 18\. November 2026, Review/);
  });
});
