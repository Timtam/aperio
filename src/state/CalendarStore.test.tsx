import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

// Mock the Tauri invoke entrypoint. The api/client module re-exports
// thin wrappers, so this is the only place we have to intercept.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
import { CalendarStoreProvider } from './CalendarStore';
import { useCalendarStore } from './calendarStoreContext';
import { buildTaskListForest } from './taskListForest';
import type { TaskList } from '../api/types';

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

function setupInvoke(handlers: Record<string, unknown[]>) {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd in handlers) {
      return Promise.resolve(handlers[cmd]);
    }
    return Promise.resolve([]);
  });
}

function Probe() {
  const store = useCalendarStore();
  return (
    <div>
      <span data-testid="cal-count">{store.calendars.length}</span>
      <span data-testid="cal-selected">
        {[...store.selectedCalendarIds].sort().join(',')}
      </span>
      <span data-testid="list-count">{store.taskLists.length}</span>
      <span data-testid="list-selected">
        {[...store.selectedTaskListIds].sort().join(',')}
      </span>
      <button type="button" onClick={() => store.toggleCalendar('a')}>
        toggle-a
      </button>
    </div>
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  localStorage.clear();
});

afterEach(() => {
  localStorage.clear();
});

describe('CalendarStoreProvider', () => {
  it('loads calendars and task lists on mount', async () => {
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'Work', color: null, read_only: false, default_sound: null },
        { id: 'b', name: 'Home', color: null, read_only: false, default_sound: null },
      ],
      list_task_lists: [
        {
          id: 'L1',
          name: 'Inbox',
          color: null,
          default_sound: null,
          embedded_in_calendar: null,
          read_only: false,
        },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('cal-count').textContent).toBe('2');
      expect(screen.getByTestId('list-count').textContent).toBe('1');
    });
  });

  it('selects everything on first run', async () => {
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'A', color: null, read_only: false, default_sound: null },
        { id: 'b', name: 'B', color: null, read_only: false, default_sound: null },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('cal-selected').textContent).toBe('a,b'),
    );
  });

  it('preserves persisted selection across reloads', async () => {
    localStorage.setItem(
      'aperio.selection.v1',
      JSON.stringify({ calendars: ['a'], taskLists: [] }),
    );
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'A', color: null, read_only: false, default_sound: null },
        { id: 'b', name: 'B', color: null, read_only: false, default_sound: null },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('cal-selected').textContent).toBe('a'),
    );
  });

  it('reconciles selection when a calendar disappears', async () => {
    localStorage.setItem(
      'aperio.selection.v1',
      JSON.stringify({ calendars: ['a', 'gone'], taskLists: [] }),
    );
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'A', color: null, read_only: false, default_sound: null },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('cal-selected').textContent).toBe('a'),
    );
  });

  it('auto-selects a newly-added calendar on the next refresh', async () => {
    // Seeded with one calendar already known + selected. A second
    // calendar appearing on a later refresh must auto-tick — that's
    // the "I added a new account and nothing showed up in the
    // sidebar" bug the known-tracking solves.
    localStorage.setItem(
      'aperio.selection.v1',
      JSON.stringify({
        calendars: ['a'],
        taskLists: [],
        knownCalendarIds: ['a'],
      }),
    );
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'A', color: null, read_only: false, default_sound: null },
        { id: 'b', name: 'B', color: null, read_only: false, default_sound: null },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('cal-selected').textContent).toBe('a,b'),
    );
  });

  it('respects an explicit untick: previously-known and unticked stays unticked', async () => {
    // The flip side of the previous test: 'b' is in known (the user
    // has seen it before) but NOT in calendars (user unticked it).
    // It must stay unticked on the next refresh even though it's
    // still present in the list — otherwise every refresh would
    // overrule the user's explicit choice.
    localStorage.setItem(
      'aperio.selection.v1',
      JSON.stringify({
        calendars: ['a'],
        taskLists: [],
        knownCalendarIds: ['a', 'b'],
      }),
    );
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'A', color: null, read_only: false, default_sound: null },
        { id: 'b', name: 'B', color: null, read_only: false, default_sound: null },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('cal-selected').textContent).toBe('a'),
    );
  });

  it('migrates pre-known-tracking localStorage without surprise-selecting', async () => {
    // Persisted blob is shaped the way the old reconciler wrote it:
    // a `calendars` selection list, but no `knownCalendarIds` field.
    // The upgrade path must freeze known to (selection ∪ list-ids)
    // so any currently-visible-but-unselected calendar stays
    // unselected — would otherwise look like a regression to
    // long-time users who had silently unticked some calendars
    // under the old "empty means select-everything" behaviour.
    localStorage.setItem(
      'aperio.selection.v1',
      JSON.stringify({ calendars: ['a'], taskLists: [] }),
    );
    setupInvoke({
      list_calendars: [
        { id: 'a', name: 'A', color: null, read_only: false, default_sound: null },
        { id: 'b', name: 'B', color: null, read_only: false, default_sound: null },
      ],
    });

    render(
      <CalendarStoreProvider>
        <Probe />
      </CalendarStoreProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('cal-selected').textContent).toBe('a'),
    );
  });
});

describe('buildTaskListForest', () => {
  const mk = (id: string, parent_id: string | null): TaskList => ({
    id,
    name: id,
    color: null,
    color_label: null,
    default_sound: null,
    embedded_in_calendar: null,
    read_only: false,
    account_id: 'acc',
    parent_id,
  });

  it('flat lists produce a depth-0 forest', () => {
    const forest = buildTaskListForest([mk('a', null), mk('b', null)]);
    expect(forest.map((n) => n.list.id)).toEqual(['a', 'b']);
    expect(forest.every((n) => n.depth === 0 && n.children.length === 0)).toBe(
      true,
    );
  });

  it('nests children under their parent with increasing depth', () => {
    const forest = buildTaskListForest([
      mk('root', null),
      mk('child', 'root'),
      mk('grandchild', 'child'),
    ]);
    expect(forest).toHaveLength(1);
    expect(forest[0].list.id).toBe('root');
    expect(forest[0].children[0].list.id).toBe('child');
    expect(forest[0].children[0].depth).toBe(1);
    expect(forest[0].children[0].children[0].list.id).toBe('grandchild');
    expect(forest[0].children[0].children[0].depth).toBe(2);
  });

  it('promotes a list whose parent is absent to a root', () => {
    const forest = buildTaskListForest([mk('orphan', 'missing')]);
    expect(forest.map((n) => n.list.id)).toEqual(['orphan']);
    expect(forest[0].depth).toBe(0);
  });

  it('does not drop lists trapped in a parent cycle', () => {
    const forest = buildTaskListForest([mk('a', 'b'), mk('b', 'a')]);
    const ids = forest.flatMap((n) => [
      n.list.id,
      ...n.children.map((c) => c.list.id),
    ]);
    expect(ids).toContain('a');
    expect(ids).toContain('b');
  });
});
