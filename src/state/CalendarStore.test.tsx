import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

// Mock the Tauri invoke entrypoint. The api/client module re-exports
// thin wrappers, so this is the only place we have to intercept.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
import { CalendarStoreProvider, useCalendarStore } from './CalendarStore';

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
});
