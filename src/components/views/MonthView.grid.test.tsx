import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent } from '../../api/types';
import type { WeekStart } from '../../state/viewMath';

/**
 * What the month grid draws. It reads its events for every day it shows: the
 * grid begins on the week start, and the range it fetched began on Monday
 * whatever that was, so with a Sunday or Saturday start the days at the
 * grid's edges stayed empty. And its week column goes with the switch
 * `view.weekNumbers`, the all-day bars keeping to their days either way.
 * Paged from outside the grid, it says the month it now shows (TODO B15
 * step 3); in the grid the new cell names its date, and with it the month.
 */

const h = vi.hoisted(() => {
  const viewState = {
    anchor: new Date(2026, 4, 15),
    pageTurn: 0,
    weekStartsOn: 1 as WeekStart,
    weekNumbers: true,
    // Moves the anchor in place: the next render, whatever triggers it,
    // reads the new day (Tab re-renders through its own selection).
    setAnchor: (d: Date) => {
      viewState.anchor = d;
    },
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
  const announce = vi.fn();
  return { viewState, ranges, store, dialogState, events, tasks, announce };
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
vi.mock('../../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));
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
  h.viewState.anchor = new Date(2026, 4, 15);
  h.viewState.pageTurn = 0;
  h.announce.mockReset();
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
    // Seen, not heard: NVDA in the grid reads the cell, and the headers are
    // hidden from it.
    const headers = [...container.querySelectorAll('[role="rowheader"]')];
    expect(headers.every((cell) => cell.getAttribute('aria-hidden') === 'true')).toBe(true);
    expect(screen.queryAllByRole('rowheader')).toEqual([]);
    // The weekday headings and the week column's head too.
    expect(container.querySelectorAll('[role="columnheader"]')).toHaveLength(8);
    expect(screen.queryAllByRole('columnheader')).toEqual([]);
    const rows = headers.map((cell) => cell.textContent);
    expect(rows).toEqual(['18', '19', '20', '21', '22']);
    expect(container.querySelector('.month-grid--no-weeks')).toBeNull();
    // Wednesday is the third day: column 4 behind the week column.
    expect(bar.style.gridColumn).toBe('4 / 6');
  });

  it('is gone with weeks switched off, and the bars keep to their days', async () => {
    h.viewState.weekNumbers = false;
    const { container, bar } = await renderMonth();
    expect(container.querySelectorAll('[role="rowheader"]')).toHaveLength(0);
    expect(container.querySelectorAll('[role="columnheader"]')).toHaveLength(7);
    expect(container.querySelector('.month-grid--no-weeks')).not.toBeNull();
    expect(bar.style.gridColumn).toBe('3 / 5');
  });
});

describe('a page turn in the month view', () => {
  async function renderMonth() {
    const { MonthView } = await import('./MonthView');
    return render(<MonthView />);
  }
  function turnTo(anchor: Date) {
    h.viewState.anchor = anchor;
    h.viewState.pageTurn += 1;
  }

  it('says the month when paged from outside the grid', async () => {
    const view = await renderMonth();
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    outside.focus();
    turnTo(new Date(2026, 5, 15));
    const { MonthView } = await import('./MonthView');
    view.rerender(<MonthView />);
    expect(h.announce).toHaveBeenCalledWith('Juni 2026');
  });

  it('leaves it to the new cell with the focus in the grid', async () => {
    const view = await renderMonth();
    screen.getByRole('grid').focus();
    turnTo(new Date(2026, 5, 15));
    const { MonthView } = await import('./MonthView');
    view.rerender(<MonthView />);
    expect(h.announce).not.toHaveBeenCalled();
  });
});

describe('the week in the name of a month cell', () => {
  /** The name of the cell the grid reports as active. */
  function activeCellName(): string {
    const grid = screen.getByRole('grid');
    const id = grid.getAttribute('aria-activedescendant');
    const cell = id ? document.getElementById(id) : null;
    if (!cell) throw new Error('the grid reports no active cell');
    return cell.getAttribute('aria-label') ?? '';
  }
  /** A step in the grid: the anchor moves with the focus there. */
  async function stepTo(view: { rerender: (ui: React.ReactElement) => void }, anchor: Date) {
    const { MonthView } = await import('./MonthView');
    h.viewState.anchor = anchor;
    view.rerender(<MonthView />);
  }

  it('says the week when a step crossed into another, and only then', async () => {
    const { MonthView } = await import('./MonthView');
    const view = render(<MonthView />);
    screen.getByRole('grid').focus();
    // Friday, 15 May; ↓ goes to Friday, 22 May, in the next week.
    await stepTo(view, new Date(2026, 4, 22));
    expect(activeCellName()).toBe('Freitag, 22. Mai 2026, Kalenderwoche 21, 0 Einträge');
    // → to Saturday stays in that week.
    await stepTo(view, new Date(2026, 4, 23));
    expect(activeCellName()).toBe('Samstag, 23. Mai 2026, 0 Einträge');
    // → again, past Sunday, crosses into the next.
    await stepTo(view, new Date(2026, 4, 25));
    expect(activeCellName()).toBe('Montag, 25. Mai 2026, Kalenderwoche 22, 0 Einträge');
  });

  it('not on entering the grid, and not with weeks switched off', async () => {
    const { MonthView } = await import('./MonthView');
    const view = render(<MonthView />);
    expect(activeCellName()).toBe('Freitag, 15. Mai 2026, 0 Einträge');
    h.viewState.weekNumbers = false;
    screen.getByRole('grid').focus();
    await stepTo(view, new Date(2026, 4, 22));
    expect(activeCellName()).toBe('Freitag, 22. Mai 2026, 0 Einträge');
  });
});

describe('the week in a month cell, beyond plain steps', () => {
  function activeCell(): HTMLElement {
    const grid = screen.getByRole('grid');
    const id = grid.getAttribute('aria-activedescendant');
    const cell = id ? document.getElementById(id) : null;
    if (!cell) throw new Error('the grid reports no active cell');
    return cell;
  }
  function meeting(id: string, title: string, month: number, day: number): CalendarEvent {
    return {
      id,
      calendar_id: 'work',
      title,
      start: new Date(2026, month - 1, day, 10, 0).toISOString(),
      end: new Date(2026, month - 1, day, 11, 0).toISOString(),
      all_day: false,
    } as unknown as CalendarEvent;
  }
  async function renderMonth() {
    const { MonthView } = await import('./MonthView');
    const view = render(<MonthView />);
    const rerender = () => act(() => view.rerender(<MonthView />));
    return { rerender };
  }
  function pageTo(anchor: Date) {
    h.viewState.anchor = anchor;
    h.viewState.pageTurn += 1;
  }
  function outsideButton() {
    const button = document.createElement('button');
    document.body.appendChild(button);
    return button;
  }

  it('paged from outside the grid: the month is said, the cell holds no week', async () => {
    const { rerender } = await renderMonth();
    act(() => outsideButton().focus());
    pageTo(new Date(2026, 5, 15));
    rerender();
    expect(h.announce).toHaveBeenCalledWith('Juni 2026');
    act(() => screen.getByRole('grid').focus());
    expect(activeCell().getAttribute('aria-label')).toBe('Montag, 15. Juni 2026, 0 Einträge');
  });

  it('paged in the grid: the new cell names the week, nothing else is said', async () => {
    const { rerender } = await renderMonth();
    act(() => screen.getByRole('grid').focus());
    pageTo(new Date(2026, 5, 15));
    rerender();
    expect(activeCell().getAttribute('aria-label')).toBe(
      'Montag, 15. Juni 2026, Kalenderwoche 25, 0 Einträge',
    );
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('once on coming back to the grid', async () => {
    const { rerender } = await renderMonth();
    act(() => screen.getByRole('grid').focus());
    h.viewState.anchor = new Date(2026, 4, 22);
    rerender();
    expect(activeCell().getAttribute('aria-label')).toContain('Kalenderwoche 21');
    act(() => outsideButton().focus());
    act(() => screen.getByRole('grid').focus());
    expect(activeCell().getAttribute('aria-label')).toBe('Freitag, 22. Mai 2026, 0 Einträge');
  });

  it('a page turn from an entry lands on the cell, even on the same cell index', async () => {
    // 15 February and 15 March 2026 both sit at index 20 of their grids.
    h.viewState.anchor = new Date(2026, 1, 15);
    h.events.events = [meeting('feb', 'Planung', 2, 15), meeting('mar', 'Review', 3, 15)];
    const { rerender } = await renderMonth();
    const grid = screen.getByRole('grid');
    act(() => grid.focus());
    fireEvent.keyDown(grid, { key: 'Tab' });
    expect(grid.getAttribute('aria-activedescendant')).toMatch(/-ev-0$/);
    pageTo(new Date(2026, 2, 15));
    rerender();
    expect(grid.getAttribute('aria-activedescendant')).not.toMatch(/-ev-/);
    expect(activeCell().getAttribute('aria-label')).toBe(
      'Sonntag, 15. März 2026, Kalenderwoche 11, 1 Eintrag',
    );
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('a page turn from an entry onto a day with none: the cell, said once', async () => {
    // The selected entry's place does not exist on 15 March: no frame may
    // point there, or the turn reads as nothing renamed and is said twice.
    h.viewState.anchor = new Date(2026, 1, 15);
    h.events.events = [meeting('feb', 'Planung', 2, 15)];
    const { rerender } = await renderMonth();
    const grid = screen.getByRole('grid');
    act(() => grid.focus());
    fireEvent.keyDown(grid, { key: 'Tab' });
    pageTo(new Date(2026, 2, 15));
    rerender();
    expect(activeCell().getAttribute('aria-label')).toBe(
      'Sonntag, 15. März 2026, Kalenderwoche 11, 0 Einträge',
    );
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('Tab into another week names it, and Escape does not repeat it', async () => {
    // Sunday, 24 May is empty; Monday, 25 May has a meeting.
    h.viewState.anchor = new Date(2026, 4, 24);
    h.events.events = [meeting('mon', 'Planung', 5, 25)];
    await renderMonth();
    const grid = screen.getByRole('grid');
    act(() => grid.focus());
    act(() => {
      fireEvent.keyDown(grid, { key: 'Tab' });
    });
    expect(h.announce).toHaveBeenCalledWith('Montag, 25. Mai 2026, Kalenderwoche 22: Planung');
    act(() => {
      fireEvent.keyDown(grid, { key: 'Escape' });
    });
    expect(activeCell().getAttribute('aria-label')).toBe('Montag, 25. Mai 2026, 1 Eintrag');
  });
});
