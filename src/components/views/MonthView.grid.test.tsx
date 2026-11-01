import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent } from '../../api/types';
import type { WeekStart } from '../../state/viewMath';

/**
 * What the month grid draws. It reads its events for every day it shows: the
 * grid begins on the week start, and the range it fetched began on Monday
 * whatever that was, so with a Sunday or Saturday start the days at the
 * grid's edges stayed empty. And its week column goes with the switch
 * `view.weekNumbers`, the all-day bars keeping to their days either way.
 */

const h = vi.hoisted(() => {
  const viewState = {
    anchor: new Date(2026, 4, 15),
    weekStartsOn: 1 as WeekStart,
    weekNumbers: true,
    setAnchor: () => {},
    goPrev: () => {},
    goNext: () => {},
  };
  const ranges: { start: Date; end: Date }[] = [];
  const store = {
    calendars: [],
    colorLabels: [],
    sectionColorById: new Map(),
    sectionsByList: {},
    loadSections: () => {},
    selectedCalendarIds: new Set<string>(),
  };
  const dialogState = {
    openEventDialog: () => {},
    openTaskDialog: () => {},
    openCreateChooser: () => {},
    invalidateData: () => {},
    dataVersion: 0,
  };
  const events = {
    events: [] as unknown[],
    calendarById: new Map(),
    loading: false,
  };
  const tasks = { tasks: [], taskListById: new Map() };
  return { viewState, ranges, store, dialogState, events, tasks };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: () => Promise.resolve([]) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../../state/viewStateContext', () => ({ useViewState: () => h.viewState }));
vi.mock('../../state/useEvents', () => ({
  useEvents: (range: { start: Date; end: Date }) => {
    h.ranges.push(range);
    return h.events;
  },
}));
vi.mock('../../state/useEventGroups', () => ({
  useEventGroups: (events: unknown[]) => ({ groups: [], events }),
}));
vi.mock('../../state/useTasks', () => ({ useTasks: () => h.tasks }));
vi.mock('../../state/taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({ visualEffortSizing: false, priorityScale: 'three' }),
}));
vi.mock('../../state/currentUser', () => ({ useCurrentUserByList: () => ({}) }));
vi.mock('../../state/useTaskListShowCompleted', () => ({
  useTaskListShowCompleted: () => ({ shouldShow: () => false }),
}));
vi.mock('../../state/useTaskStatusToggle', () => ({ useTaskStatusToggle: () => () => {} }));
vi.mock('../../state/calendarStoreContext', () => ({ useCalendarStore: () => h.store }));
vi.mock('../../state/useChipContextMenu', () => ({
  useChipContextMenu: () => ({
    openForEvent: () => {},
    openForTask: () => {},
    openForTaskProjection: () => {},
  }),
}));
vi.mock('../../state/dialogStateContext', () => ({ useDialogState: () => h.dialogState }));
vi.mock('../../state/useDayLogSummaries', () => ({
  useDayLogSummaries: () => ({ symbolsFor: () => '', spokenFor: () => '' }),
}));
vi.mock('../../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../DayCheckInButton', () => ({ DayCheckInButton: () => null }));
vi.mock('../BacklogRail', () => ({ BacklogRail: () => null }));
vi.mock('../ExceptionsLossQuestion', () => ({
  useExceptionsLossQuestion: () => ({ ask: async () => true, dialog: null }),
}));

afterEach(() => {
  document.body.innerHTML = '';
  h.ranges.length = 0;
  h.viewState.weekStartsOn = 1;
  h.viewState.weekNumbers = true;
  h.events.events = [];
});

const day = (y: number, m: number, d: number) => new Date(y, m - 1, d);

async function rangeFor(weekStartsOn: WeekStart) {
  h.viewState.weekStartsOn = weekStartsOn;
  const { MonthView } = await import('./MonthView');
  render(<MonthView />);
  const range = h.ranges.at(-1);
  if (range == null) throw new Error('the month view read no events');
  return range;
}

describe('the month view reads every day its grid draws', () => {
  // May 2026 begins on a Friday and ends on a Sunday.
  it('with the week starting on Sunday', async () => {
    const range = await rangeFor(0);
    // The grid opens on Sunday, 26 April, and closes on Saturday, 6 June.
    expect(range.start.getTime()).toBe(day(2026, 4, 26).getTime());
    expect(range.end.getTime()).toBeGreaterThanOrEqual(day(2026, 6, 6).getTime());
    expect(range.end.getTime()).toBeLessThan(day(2026, 6, 7).getTime());
  });

  it('with the week starting on Saturday', async () => {
    const range = await rangeFor(6);
    // The grid opens on Saturday, 25 April, and closes on Friday, 5 June.
    expect(range.start.getTime()).toBe(day(2026, 4, 25).getTime());
    expect(range.end.getTime()).toBeGreaterThanOrEqual(day(2026, 6, 5).getTime());
    expect(range.end.getTime()).toBeLessThan(day(2026, 6, 6).getTime());
  });

  it('with the week starting on Monday', async () => {
    const range = await rangeFor(1);
    expect(range.start.getTime()).toBe(day(2026, 4, 27).getTime());
    expect(range.end.getTime()).toBeLessThan(day(2026, 6, 1).getTime());
  });
});

describe('the week column of the month grid', () => {
  // Wednesday, 13 May, to Thursday, 14 May 2026, in the grid's third week.
  const conference = {
    id: 'conference',
    calendar_id: 'work',
    title: 'Konferenz',
    start: new Date(2026, 4, 13).toISOString(),
    end: new Date(2026, 4, 15).toISOString(),
    all_day: true,
  } as unknown as CalendarEvent;

  async function renderMonth() {
    h.events.events = [conference];
    const { MonthView } = await import('./MonthView');
    const { container } = render(<MonthView />);
    const bar = container.querySelector<HTMLElement>('.month-allday-bar');
    if (!bar) throw new Error('the month view drew no all-day bar');
    return { container, bar };
  }

  it('numbers each row, and the bars start one column in', async () => {
    const { container, bar } = await renderMonth();
    const rows = screen.getAllByRole('rowheader').map((cell) => cell.textContent);
    expect(rows).toEqual(['18', '19', '20', '21', '22']);
    expect(container.querySelector('.month-grid--no-weeks')).toBeNull();
    // Wednesday is the third day: column 4 behind the week column.
    expect(bar.style.gridColumn).toBe('4 / 6');
  });

  it('is gone with weeks switched off, and the bars keep to their days', async () => {
    h.viewState.weekNumbers = false;
    const { container, bar } = await renderMonth();
    expect(screen.queryAllByRole('rowheader')).toEqual([]);
    expect(screen.getAllByRole('columnheader')).toHaveLength(7);
    expect(container.querySelector('.month-grid--no-weeks')).not.toBeNull();
    expect(bar.style.gridColumn).toBe('3 / 5');
  });
});
