import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';

import type { Task } from '../api/types';

/**
 * Mirror of useEvents.test.tsx for the task-list selection key: on a
 * selection change the first render must show the new selection's cached
 * batch (warm) or nothing (cold) — never the previous selection's tasks.
 */

const getTasksMock = vi.hoisted(() =>
  vi.fn((listId: string) => Promise.resolve(FIXTURES[listId] ?? [])),
);
vi.mock('../api/client', () => ({ getTasks: getTasksMock }));

const selection = vi.hoisted(() => ({ ids: new Set(['l1']) }));
vi.mock('./calendarStoreContext', () => ({
  useCalendarStore: () => ({
    selectedTaskListIds: selection.ids,
    taskLists: [],
    taskListsLoading: false,
  }),
}));
vi.mock('./dialogStateContext', () => ({
  useDialogState: () => ({ dataVersion: 0 }),
}));

import { __resetTasksCacheForTests, useTasks } from './useTasks';

function task(id: string): Task {
  return {
    id,
    list_id: 'l1',
    title: id,
    status: 'open',
    created_at: '2026-06-01T00:00:00.000Z',
  } as unknown as Task;
}

const FIXTURES: Record<string, Task[]> = {
  l1: [task('t1'), task('t2')],
  l2: [task('t3')],
};

function renderLogged() {
  const log: { loading: boolean; ids: string[] }[] = [];
  const hook = renderHook(() => {
    const r = useTasks();
    log.push({ loading: r.loading, ids: r.tasks.map((x) => x.id).sort() });
    return r;
  });
  return { ...hook, log };
}

beforeEach(() => {
  __resetTasksCacheForTests();
  getTasksMock.mockClear();
  selection.ids = new Set(['l1']);
});

describe('useTasks selection changes', () => {
  it('renders a warm selection from the cache on the first render after switching back', async () => {
    const { result, rerender, log } = renderLogged();
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.tasks.map((x) => x.id).sort()).toEqual(['t1', 't2']);

    selection.ids = new Set(['l2']);
    rerender();
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.tasks.map((x) => x.id)).toEqual(['t3']);

    selection.ids = new Set(['l1']);
    const before = log.length;
    rerender();
    expect(log[before]).toEqual({ loading: false, ids: ['t1', 't2'] });
    await act(async () => {});
  });

  it('renders a cold selection as empty and loading, never the previous one', async () => {
    const { result, rerender, log } = renderLogged();
    await waitFor(() => expect(result.current.loading).toBe(false));

    selection.ids = new Set(['l3']);
    const before = log.length;
    rerender();
    expect(log[before]).toEqual({ loading: true, ids: [] });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.tasks).toHaveLength(0);
  });
});
