import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';
import type { Task, TaskList } from '../api/types';

/**
 * The sentences a check-off says, in the real German the app speaks (the
 * other test file reads keys): a check-off whose related task failed said the
 * raw key "views.tasks.cascadePartial", since only its _one/_other forms
 * exist and no count was passed, and it never said why or to try later.
 */

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const announce = vi.fn();
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('./dialogStateContext', () => ({ useDialogState: () => ({ invalidateData: vi.fn() }) }));
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

const task = (id: string, title: string, parent_id: string | null = null): Task =>
  ({
    id,
    list_id: 'L1',
    title,
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
  }) as Task;

const TASKS = [task('root', 'Umzug'), task('kid', 'Kartons', 'root')];
vi.mock('./useTasks', () => ({
  useTasks: () => ({ tasks: TASKS, taskListById: new Map([['L1', LIST]]) }),
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

describe('a check-off whose related task failed, as said', () => {
  it('counts the failed ones and says the provider is busy and to try later', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({})
      .mockRejectedValueOnce({ code: 'forbidden', message: 'provider-busy: http-429' });
    const { result } = renderHook(() => useTaskStatusActions());
    await act(async () => {
      await result.current.set(TASKS[0], 'completed');
    });
    const said = announce.mock.calls[0][0] as string;
    expect(said).not.toMatch(/views\.tasks/);
    expect(said).toContain('0 von 1 weiteren Aufgaben mit aktualisiert, eine fehlgeschlagen.');
    expect(said).toContain('ausgelastet');
    expect(said).toMatch(/später noch einmal/);
  });
});
