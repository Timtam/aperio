import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Calendar, CalendarEvent } from '../../api/types';
import type { WeekStart } from '../../state/viewMath';

/**
 * What a screen reader hears of the calendar week in the week view (TODO
 * B15). NVDA reads only the cell it lands on, so the grid's name carries the
 * week on entry and a cell carries it only when a step in the grid crossed
 * into another week — ↓ moves a whole week, the case Toni heard nothing in.
 * The name then holds until the next step, or a reload of the items would
 * make NVDA read the cell again. A step from outside the grid is heard on
 * entering it, through the grid's own name.
 */

const h = vi.hoisted(() => {
  type State = {
    anchor: Date;
    weekStartsOn: WeekStart;
    weekNumbers: boolean;
  };
  let state: State = {
    anchor: new Date(2026, 10, 4),
    weekStartsOn: 1,
    weekNumbers: true,
  };
  const listeners = new Set<() => void>();
  const set = (patch: Partial<State>) => {
    state = { ...state, ...patch };
    listeners.forEach((l) => l());
  };
  const shift = (days: number) => {
    const next = new Date(state.anchor);
    next.setDate(next.getDate() + days);
    set({ anchor: next });
  };
  const viewState = {
    get: () => state,
    set,
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    // The value `useViewState` hands out, rebuilt only when the state moves
    // (`useSyncExternalStore` wants a stable snapshot).
    snapshot: null as unknown,
    snapshotOf: null as State | null,
    actions: {
      setAnchor: (d: Date) => set({ anchor: d }),
      goPrev: () => shift(-7),
      goNext: () => shift(7),
    },
  };
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
  const noEvents = { events: [] as unknown[], calendarById: new Map(), loading: false };
  const events = { current: noEvents, none: noEvents };
  const tasks = { tasks: [], taskListById: new Map() };
  return { viewState, store, dialogState, events, tasks };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: () => Promise.resolve([]) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../../state/viewStateContext', async () => {
  const { useSyncExternalStore } = await import('react');
  const vs = h.viewState;
  const snapshot = () => {
    const state = vs.get();
    if (vs.snapshotOf !== state) {
      vs.snapshotOf = state;
      vs.snapshot = { ...state, ...vs.actions };
    }
    return vs.snapshot;
  };
  return { useViewState: () => useSyncExternalStore(vs.subscribe, snapshot) };
});
vi.mock('../../state/useEvents', () => ({ useEvents: () => h.events.current }));
vi.mock('../../state/useEventGroups', () => ({
  useEventGroups: (events: unknown[]) => ({ groups: [], events }),
}));
vi.mock('../../state/useTasks', () => ({ useTasks: () => h.tasks }));
vi.mock('../../state/taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({
    visualEffortSizing: false,
    priorityScale: 'three',
    dayViewMode: 'list',
  }),
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

beforeEach(() => {
  h.viewState.set({ anchor: new Date(2026, 10, 4), weekStartsOn: 1, weekNumbers: true });
  h.events.current = h.events.none;
});

const WORK = { id: 'work', name: 'Arbeit', read_only: false } as unknown as Calendar;
function meeting(id: string, day: number): CalendarEvent {
  return {
    id,
    calendar_id: WORK.id,
    title: `Planung ${day}`,
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

afterEach(() => {
  document.body.innerHTML = '';
});

async function renderWeek() {
  const { WeekView } = await import('./WeekView');
  const view = render(<WeekView />);
  return { view, WeekView };
}

/** The grid, and the name of the cell it reports as active. */
function grid() {
  return screen.getByRole('grid');
}
function activeCellName(): string {
  const id = grid().getAttribute('aria-activedescendant');
  const cell = id ? document.getElementById(id) : null;
  if (!cell) throw new Error('the grid reports no active cell');
  return cell.getAttribute('aria-label') ?? '';
}
/** A key in the grid, with the focus there as it is for a keyboard user. */
function press(key: string) {
  grid().focus();
  fireEvent.keyDown(grid(), { key });
}

describe('the week view says the calendar week', () => {
  it('in the grid name on entry, and the heading speaks it in words', async () => {
    await renderWeek();
    expect(grid()).toHaveAttribute(
      'aria-label',
      'Wochenraster, Kalenderwoche 45, 2. bis 8. November 2026',
    );
    const heading = screen.getByRole('heading', { level: 2 });
    expect(heading).toHaveAccessibleName('Kalenderwoche 45, 2. bis 8. November 2026');
    // The eye still reads the short form.
    expect(heading.textContent).toContain('KW 45 · ');
    // Entering names the week once, in the grid; the cell does not repeat it.
    expect(activeCellName()).toBe('Mittwoch, 4. November 2026, 0 Einträge');
  });

  it('in the cell a step lands on when it crossed into another week, and only then', async () => {
    await renderWeek();
    press('ArrowRight');
    expect(activeCellName()).toBe('Donnerstag, 5. November 2026, 0 Einträge');
    press('ArrowDown');
    expect(activeCellName()).toBe(
      'Donnerstag, 12. November 2026, Kalenderwoche 46, 0 Einträge',
    );
    press('ArrowRight');
    expect(activeCellName()).toBe('Freitag, 13. November 2026, 0 Einträge');
  });

  it('across the week start by day, backwards too', async () => {
    h.viewState.set({ anchor: new Date(2026, 10, 2) });
    await renderWeek();
    press('ArrowLeft');
    expect(activeCellName()).toBe(
      'Sonntag, 1. November 2026, Kalenderwoche 44, 0 Einträge',
    );
  });

  it('keeps the name through a reload of the items until the next step', async () => {
    const { view, WeekView } = await renderWeek();
    press('ArrowDown');
    const named = activeCellName();
    expect(named).toContain('Kalenderwoche 46');
    // The items arrive again: a new answer object, the same day.
    act(() => {
      h.events.current = { ...h.events.current };
    });
    view.rerender(<WeekView />);
    expect(activeCellName()).toBe(named);
  });

  it('not in the cell when the week was paged from outside the grid', async () => {
    await renderWeek();
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    outside.focus();
    // The toolbar's "next" button, say.
    act(() => h.viewState.actions.goNext());
    expect(grid()).toHaveAttribute(
      'aria-label',
      'Wochenraster, Kalenderwoche 46, 9. bis 15. November 2026',
    );
    expect(activeCellName()).toBe('Mittwoch, 11. November 2026, 0 Einträge');
  });

  it('from an entry too: a week step lands on the day, which says the week', async () => {
    // A meeting on Wednesday of this week and of the next.
    h.events.current = {
      events: [meeting('a', 4), meeting('b', 11)],
      calendarById: new Map([[WORK.id, WORK]]),
      loading: false,
    };
    await renderWeek();
    press('Tab');
    expect(activeCellName()).toContain('Planung 4');
    press('ArrowDown');
    expect(activeCellName()).toBe(
      'Mittwoch, 11. November 2026, Kalenderwoche 46, 1 Eintrag',
    );
  });

  it('once on coming back: the grid says it, the cell no longer', async () => {
    await renderWeek();
    press('ArrowDown');
    expect(activeCellName()).toContain('Kalenderwoche 46');
    // Out to the toolbar and back in, or a dialog opened and closed.
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    act(() => outside.focus());
    act(() => grid().focus());
    expect(activeCellName()).toBe('Mittwoch, 11. November 2026, 0 Einträge');
  });

  it('switched off, neither seen nor heard', async () => {
    h.viewState.set({ weekNumbers: false });
    await renderWeek();
    expect(grid()).toHaveAttribute('aria-label', 'Wochenraster');
    const heading = screen.getByRole('heading', { level: 2 });
    expect(heading).toHaveAccessibleName('2. bis 8. November 2026');
    expect(heading.textContent).not.toContain('KW');
    press('ArrowDown');
    expect(activeCellName()).toBe('Mittwoch, 11. November 2026, 0 Einträge');
  });
});
