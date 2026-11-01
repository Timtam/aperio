import { render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { WeekStart } from '../../state/viewMath';

/**
 * The month view reads its events for every day its grid draws. The grid
 * begins on the week start, and the range it fetched began on Monday
 * whatever that was: with a Sunday or Saturday start the days at the grid's
 * edges stayed empty.
 */

const h = vi.hoisted(() => {
  const viewState = {
    anchor: new Date(2026, 4, 15),
    weekStartsOn: 1 as WeekStart,
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
  const events = { events: [], calendarById: new Map(), loading: false };
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
