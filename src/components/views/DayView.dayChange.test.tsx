import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Calendar, CalendarEvent } from '../../api/types';
import type { WeekStart } from '../../state/viewMath';

/**
 * What a day change says in the day view (TODO B15 step 3). NVDA in the list
 * reads only the active entry, which keeps its place when the day changes, so
 * after a day change with the focus in the list that entry names the new
 * date in front, and the week with it when the day lies in another week.
 * Paged from outside the list, or onto an empty day, the day is announced.
 */

const h = vi.hoisted(() => {
  type State = {
    anchor: Date;
    pageTurn: number;
    weekStartsOn: WeekStart;
    weekNumbers: boolean;
  };
  let state: State = {
    anchor: new Date(2026, 10, 3),
    pageTurn: 0,
    weekStartsOn: 1,
    weekNumbers: true,
  };
  const listeners = new Set<() => void>();
  const set = (patch: Partial<State>) => {
    state = { ...state, ...patch };
    listeners.forEach((l) => l());
  };
  const page = (days: number) => {
    const anchor = new Date(state.anchor);
    anchor.setDate(anchor.getDate() + days);
    set({ anchor, pageTurn: state.pageTurn + 1 });
  };
  const viewState = {
    get: () => state,
    set,
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    snapshot: null as unknown,
    snapshotOf: null as State | null,
    actions: {
      setAnchor: (d: Date) => set({ anchor: d }),
      goPrev: () => page(-1),
      goNext: () => page(1),
      jumpToToday: () => page(0),
    },
  };
  const store = {
    calendars: [],
    colorLabels: [],
    sectionColorById: new Map(),
    sectionsByList: {},
    loadSections: () => {},
    taskLists: [],
    selectedCalendarIds: new Set<string>(),
  };
  const dialogState = {
    openEventDialog: () => {},
    openTaskDialog: () => {},
    openMoveCopy: () => {},
    openCreateChooser: () => {},
    invalidateData: () => {},
    stepWriteWait: () => async <T,>(_inSteps: boolean, write: () => Promise<T>) => write(),
    dataVersion: 0,
  };
  const noEvents = { events: [] as unknown[], calendarById: new Map(), loading: false };
  const events = { current: noEvents };
  const tasks = { tasks: [], taskListById: new Map() };
  const announce = vi.fn();
  return { viewState, store, dialogState, events, noEvents, tasks, announce };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: () => Promise.resolve([]) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  getDayLog: () => Promise.resolve(null),
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
    dayViewMode: 'list',
    dayStartMin: 0,
    dayEndMin: 1440,
    priorityScale: 'three',
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
vi.mock('../../state/useDayMarkers', () => ({ useDayMarkers: () => ({ markers: [] }) }));
vi.mock('../../state/dayMarkersChanged', () => ({ useDayMarkersChanged: () => {} }));
vi.mock('../../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));
vi.mock('../GroupSuggestionNotice', () => ({ GroupSuggestionNotice: () => null }));
vi.mock('../DayLogDialog', () => ({ DayLogDialog: () => null }));
vi.mock('../ExceptionsLossQuestion', () => ({
  useExceptionsLossQuestion: () => ({ ask: async () => true, dialog: null }),
}));

const WORK = { id: 'work', name: 'Arbeit', read_only: false } as unknown as Calendar;
function meeting(id: string, title: string, day: number, hour = 10): CalendarEvent {
  return {
    id,
    calendar_id: WORK.id,
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

/** Tuesday 3, Wednesday 4 (two meetings), Monday 9 November; Sunday 8 empty. */
const MEETINGS = [
  meeting('tue', 'Planung', 3),
  meeting('wed1', 'Review', 4, 9),
  meeting('wed2', 'Retro', 4, 14),
  meeting('mon', 'Wochenstart', 9),
];

beforeEach(() => {
  h.viewState.set({
    anchor: new Date(2026, 10, 3),
    pageTurn: 0,
    weekStartsOn: 1,
    weekNumbers: true,
  });
  h.events.current = {
    events: MEETINGS,
    calendarById: new Map([[WORK.id, WORK]]),
    loading: false,
  };
  h.announce.mockReset();
  document.body.innerHTML = '';
});

async function renderDay() {
  const { DayView } = await import('./DayView');
  const view = render(<DayView />);
  // Let the day log's read settle inside act.
  await act(async () => {});
  return { view, DayView };
}

function list() {
  return screen.getByRole('listbox');
}
function activeName(): string {
  const id = list().getAttribute('aria-activedescendant');
  const option = id ? document.getElementById(id) : null;
  if (!option) throw new Error('the list reports no active entry');
  return option.getAttribute('aria-label') ?? '';
}
/** A page turn: Back or Forward, or Today on the day it stands on. */
async function page(direction: 'back' | 'forward' | 'today') {
  const { goPrev, goNext, jumpToToday } = h.viewState.actions;
  await act(async () => {
    if (direction === 'back') goPrev();
    else if (direction === 'forward') goNext();
    else jumpToToday();
  });
}
/** A page turn made with the focus in the list (Ctrl+→, say). */
async function pageInList(direction: 'back' | 'forward') {
  act(() => list().focus());
  await page(direction);
}
/** The focus on a button outside the list, as on the toolbar. */
function focusOutside() {
  const outside = document.createElement('button');
  document.body.appendChild(outside);
  act(() => outside.focus());
}

describe('a day change in the day view', () => {
  it('puts the new date in front of the active entry', async () => {
    await renderDay();
    expect(activeName()).toMatch(/^Termin: Planung/);
    await pageInList('forward');
    expect(activeName()).toMatch(/^Mittwoch, 4\. November 2026, Termin: Review/);
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('and the week, when the day lies in another week', async () => {
    h.viewState.set({ anchor: new Date(2026, 10, 8) });
    await renderDay();
    await pageInList('forward');
    expect(activeName()).toMatch(
      /^Montag, 9\. November 2026, Kalenderwoche 46, Termin: Wochenstart/,
    );
  });

  it('only until the user moves to another entry', async () => {
    await renderDay();
    await pageInList('forward');
    fireEvent.keyDown(list(), { key: 'ArrowDown' });
    expect(activeName()).toMatch(/^Termin: Retro/);
    fireEvent.keyDown(list(), { key: 'ArrowUp' });
    expect(activeName()).toMatch(/^Termin: Review/);
  });

  it('keeps it through a reload of the items', async () => {
    const { view, DayView } = await renderDay();
    await pageInList('forward');
    const named = activeName();
    h.events.current = { ...h.events.current };
    view.rerender(<DayView />);
    expect(activeName()).toBe(named);
  });

  it('is announced when paged from outside the list', async () => {
    await renderDay();
    focusOutside();
    await page('forward');
    expect(h.announce).toHaveBeenCalledWith('Mittwoch, 4. November 2026');
    expect(activeName()).toMatch(/^Termin: Review/);
  });

  it('keeps the date when a key cannot move', async () => {
    await renderDay();
    await pageInList('forward');
    const named = activeName();
    // ↑ on the first entry: nothing moves, so nothing is re-read.
    fireEvent.keyDown(list(), { key: 'ArrowUp' });
    expect(activeName()).toBe(named);
  });

  it('says Today on today as the date alone, not with an older week', async () => {
    h.viewState.set({ anchor: new Date(2026, 10, 9) });
    await renderDay();
    focusOutside();
    await page('back');
    expect(h.announce).toHaveBeenLastCalledWith('Sonntag, 8. November 2026, Kalenderwoche 45');
    await page('today');
    expect(h.announce).toHaveBeenLastCalledWith('Sonntag, 8. November 2026');
  });

  it('waits for a day that is still loading, then says it if it stays empty', async () => {
    h.viewState.set({ anchor: new Date(2026, 10, 9) });
    const { view, DayView } = await renderDay();
    h.events.current = { ...h.events.current, events: [], loading: true };
    await pageInList('back');
    expect(h.announce).not.toHaveBeenCalled();
    // The day's read answers: nothing on Sunday.
    h.events.current = { ...h.events.current, loading: false };
    await act(async () => view.rerender(<DayView />));
    expect(h.announce).toHaveBeenCalledTimes(1);
    expect(h.announce).toHaveBeenCalledWith('Sonntag, 8. November 2026, Kalenderwoche 45');
  });

  it('says nothing of a day when its last entry goes later', async () => {
    const { view, DayView } = await renderDay();
    await pageInList('forward');
    h.events.current = { ...h.events.current, events: [] };
    await act(async () => view.rerender(<DayView />));
    expect(h.announce).not.toHaveBeenCalled();
  });

  it('is announced once when the new day has nothing to carry it', async () => {
    h.viewState.set({ anchor: new Date(2026, 10, 9) });
    await renderDay();
    await pageInList('back');
    expect(h.announce).toHaveBeenCalledTimes(1);
    expect(h.announce).toHaveBeenCalledWith('Sonntag, 8. November 2026, Kalenderwoche 45');
  });

  it('says no week with weeks switched off', async () => {
    h.viewState.set({ anchor: new Date(2026, 10, 8), weekNumbers: false });
    await renderDay();
    await pageInList('forward');
    expect(activeName()).toMatch(/^Montag, 9\. November 2026, Termin: Wochenstart/);
  });
});
