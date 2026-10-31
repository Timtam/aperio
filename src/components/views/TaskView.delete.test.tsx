import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import { writeErrorMessage } from '@aperio/shared';

import type { Task, TaskList } from '../../api/types';
import i18n from '../../i18n';

/**
 * The Tasks view's own Delete key, after a delete that failed, reads the list
 * again (decision 277), as the row menu's Delete and the phone's task list
 * do: a delete whose answer was lost may have landed, and the row would stay
 * on screen until some later refresh.
 */

const { invokeMock, invalidateData, announced } = vi.hoisted(() => ({
  invokeMock: vi.fn((command: string) =>
    command === 'delete_task'
      ? Promise.reject({ code: 'network', message: 'connection reset' })
      : Promise.resolve(null),
  ),
  invalidateData: vi.fn(),
  announced: [] as string[],
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));

const LIST = { id: 'L1', name: 'Arbeit', account_id: 'acc-caldav' } as unknown as TaskList;
const TASK = {
  id: 't-1',
  list_id: 'L1',
  parent_id: null,
  title: 'Bericht',
  description: null,
  status: 'open',
  priority: 'medium',
  due: null,
  scheduled_date: null,
  resurface_date: null,
  section_id: null,
  color_label: null,
  reminders: [],
  assignees: [],
  completed_at: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
} as unknown as Task;
const TASKS = [TASK];
const LISTS = new Map([[LIST.id, LIST]]);
vi.mock('../../state/useTasks', () => ({
  useTasks: () => ({ tasks: TASKS, taskListById: LISTS, loading: false }),
}));
const STORE = {
  colorLabels: [],
  sectionsByList: { L1: [] },
  loadSections: () => Promise.resolve(),
  sectionColorById: new Map(),
};
vi.mock('../../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../../state/taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({ visualEffortSizing: false, priorityScale: 'three' }),
}));
vi.mock('../../state/currentUser', () => ({ useCurrentUserByList: () => ({}) }));
vi.mock('../../state/useChipContextMenu', () => ({
  useChipContextMenu: () => ({
    openForEvent: () => Promise.resolve(),
    openForTask: () => Promise.resolve(),
    openForTaskProjection: () => Promise.resolve(),
  }),
}));
vi.mock('../../state/useTaskStatusToggle', () => ({ useTaskStatusToggle: () => () => {} }));
vi.mock('../../state/dialogStateContext', () => ({
  useDialogState: () => ({
    openTaskDialog: () => {},
    openMoveCopy: () => {},
    openPlanTask: () => {},
    openSectionDialog: () => {},
    invalidateData,
  }),
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  invalidateData.mockClear();
  announced.length = 0;
});

describe('TaskView → the Delete key after a delete that failed (277)', () => {
  it('says the failure and reads the list again', async () => {
    const { TaskView } = await import('./TaskView');
    render(<TaskView />);
    const tree = await screen.findByRole('tree');
    // Past the group headings onto the task, the last row.
    fireEvent.keyDown(tree, { key: 'End' });
    fireEvent.keyDown(tree, { key: 'Delete' });
    fireEvent.click(await screen.findByRole('button', { name: /^löschen$|^delete$/i }));
    await waitFor(() => expect(invalidateData).toHaveBeenCalledTimes(1));
    expect(invokeMock.mock.calls.filter((call) => call[0] === 'delete_task')).toHaveLength(1);
    const t = (key: string, values?: Record<string, unknown>): string =>
      i18n.t(key, values as never) as unknown as string;
    expect(announced).toEqual([
      writeErrorMessage({ code: 'network', message: 'connection reset' }, t),
    ]);
  });
});
