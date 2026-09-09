import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';
import type { Task, TaskList } from '../api/types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const announce = vi.fn();
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));

// The announcement is the thing under test, so `t` returns the key plus its
// interpolations rather than prose — an assertion on German text would break on
// every wording change and tell us nothing about the behaviour.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars ? `${key}(${JSON.stringify(vars)})` : key,
  }),
}));

const invalidateData = vi.fn();
vi.mock('./dialogStateContext', () => ({
  useDialogState: () => ({ invalidateData }),
}));

vi.mock('./currentUser', () => ({ currentUserForList: async () => null }));

const LIST: TaskList = {
  id: 'L1',
  name: 'List',
  color: null,
  color_label: null,
  default_sound: null,
  embedded_in_calendar: null,
  read_only: false,
  account_id: 'local',
  parent_id: null,
  task_capabilities: DEFAULT_TASK_CAPABILITIES,
};

const task = (id: string, parent_id: string | null = null): Task => ({
  id,
  list_id: 'L1',
  title: `task ${id}`,
  description: null,
  status: 'open',
  priority: 'medium',
  effort: 'medium',
  scheduled_date: null,
  scheduled_time: null,
  scheduled_end_time: null,
  deadline_date: null,
  deadline_time: null,
  deadline_reminder_days: null,
  recurrence: null,
  resurface_date: null,
  series_id: null,
  parent_id,
  section_id: null,
  color_label: null,
  reminders: [],
  sound: null,
  assignees: [],
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  completed_at: null,
  etag: null,
});

let TASKS: Task[] = [];
vi.mock('./useTasks', () => ({
  useTasks: () => ({
    tasks: TASKS,
    taskListById: new Map([['L1', LIST]]),
  }),
}));

vi.mock('./taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({
    effectiveForList: () => ({ cascade: true, autoDate: false }),
    checkoffMode: 'binary',
    autoSelfAssign: false,
  }),
}));

import { invoke } from '@tauri-apps/api/core';
import { useTaskStatusActions } from './useTaskStatusToggle';

/**
 * What a screen-reader user hears when a check-off does NOT fully work.
 *
 * The desktop used to answer a failed cascade with `console.warn` and nothing
 * else: the row was checked off, part of the family had already changed in the
 * store, and the app said nothing at all. On the surface where the aria-live
 * announcement IS the feedback, silence is the worst available outcome — worse
 * than a wrong count, because there is nothing to notice.
 */
describe('useTaskStatusActions — what the user is told', () => {
  beforeEach(() => {
    announce.mockClear();
    invalidateData.mockClear();
    vi.mocked(invoke).mockReset();
    TASKS = [task('root'), task('kid', 'root')];
  });

  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it('announces the status and the cascade count when every write lands', async () => {
    vi.mocked(invoke).mockResolvedValue({});
    const { result } = renderHook(() => useTaskStatusActions());
    await act(async () => {
      await result.current.set(TASKS[0], 'completed');
    });
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce.mock.calls[0][0]).toContain('cascadeSuffix');
    expect(announce.mock.calls[0][0]).toContain('"count":1');
  });

  it('says so when the focused task itself could not be written', async () => {
    // The root write is planned first, so this is the row the user acted on.
    vi.mocked(invoke).mockRejectedValue(new Error('provider said no'));
    const { result } = renderHook(() => useTaskStatusActions());
    await act(async () => {
      await result.current.set(TASKS[0], 'completed');
    });
    expect(announce).toHaveBeenCalledTimes(1);
    const said = announce.mock.calls[0][0] as string;
    expect(said).toContain('statusChangeFailed');
    expect(said).toContain('provider said no');
    // And it must NOT claim the task changed status.
    expect(said).not.toContain('completedAnnounce');
  });

  it('reports how much of the cascade landed when only part of it failed', async () => {
    // Root succeeds, the child does not.
    vi.mocked(invoke)
      .mockResolvedValueOnce({})
      .mockRejectedValueOnce(new Error('child rejected'));
    const { result } = renderHook(() => useTaskStatusActions());
    await act(async () => {
      await result.current.set(TASKS[0], 'completed');
    });
    const said = announce.mock.calls[0][0] as string;
    expect(said).toContain('cascadePartial');
    expect(said).toContain('"applied":0');
    expect(said).toContain('"total":1');
    expect(said).toContain('"failed":1');
  });

  it('attempts every write even after one is rejected', async () => {
    // Stopping at the first failure left an arbitrary prefix applied, and which
    // prefix depended on nothing the user could see.
    TASKS = [task('root'), task('a', 'root'), task('b', 'root')];
    vi.mocked(invoke)
      .mockResolvedValueOnce({})
      .mockRejectedValueOnce(new Error('a rejected'))
      .mockResolvedValueOnce({});
    const { result } = renderHook(() => useTaskStatusActions());
    await act(async () => {
      await result.current.set(TASKS[0], 'completed');
    });
    expect(invoke).toHaveBeenCalledTimes(3);
  });

  it('refreshes the view even when a write failed', async () => {
    // Whatever landed before the failure is real; a view still showing the old
    // state is a view that disagrees with the store.
    vi.mocked(invoke)
      .mockResolvedValueOnce({})
      .mockRejectedValueOnce(new Error('child rejected'));
    const { result } = renderHook(() => useTaskStatusActions());
    await act(async () => {
      await result.current.set(TASKS[0], 'completed');
    });
    expect(invalidateData).toHaveBeenCalled();
  });
});
