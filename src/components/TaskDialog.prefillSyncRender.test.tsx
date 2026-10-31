import { useEffect, useSyncExternalStore } from 'react';
import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';

import type { Task, TaskList } from '../api/types';

/**
 * The task twin of `EventDialog.prefillSyncRender.test.tsx`: a store the
 * editor reads pushes a change synchronously right after the editor's effects
 * ran, so the reset runs again before the prefill's update has rendered (Toni's
 * release log, 2026-10-31). Without StrictMode, as in a release build.
 */

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const list = (id: string, name: string, read_only = false) =>
  ({ id, name, read_only, task_capabilities: DEFAULT_TASK_CAPABILITIES }) as unknown as TaskList;
const INBOX = list('list-inbox', 'Eingang');
const WORK = list('list-work', 'Arbeit');
const SHARED = list('list-shared', 'Geteilt', true);

/** A bare task: filled from it, the form equals its baseline, so a catalog
 *  refresh resets it as pristine and re-applies the prefill. */
const task = (list_id: string): Task =>
  ({
    id: 'task-1',
    list_id,
    title: 'Einkaufen',
    description: null,
    priority: 'medium',
    effort: 'medium',
    color_label: null,
    reminders: [],
    recurrence: null,
    deadline_reminder_days: null,
    status: 'open',
  }) as unknown as Task;

/** A store that pushes its changes synchronously, as a warm one does. */
const store = vi.hoisted(() => {
  let snapshot: Record<string, unknown> = {};
  const listeners = new Set<() => void>();
  return {
    get: () => snapshot,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    set: (next: Record<string, unknown>) => {
      snapshot = next;
      listeners.forEach((listener) => listener());
    },
  };
});
store.set({
  taskLists: [INBOX, WORK, SHARED],
  selectedTaskListIds: new Set(['list-inbox', 'list-work', 'list-shared']),
  colorLabels: [],
  sectionsByList: {},
  sectionsWritten: {},
  sectionsReadFailure: {},
  loadSections: () => Promise.resolve(),
});
const VIEW_STATE = { showHiddenTaskListTargets: false, timeStepMinutes: 15 };
const CASCADE = { enabled: false, autoDate: false, autoSelfAssign: false, priorityScale: 'three' };
const announce = vi.hoisted(() => vi.fn());

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => useSyncExternalStore(store.subscribe, store.get),
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
const DIALOGS = vi.hoisted(() => ({ invalidateData: () => {} }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => DIALOGS }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
const TASKS = vi.hoisted(() => ({ tasks: [] }));
vi.mock('../state/useTasks', () => ({ useTasks: () => TASKS }));
const ACTIONS = vi.hoisted(() => ({ toggle: () => {}, set: () => {}, priority: () => {} }));
vi.mock('../state/useTaskStatusToggle', () => ({ useTaskStatusActions: () => ACTIONS }));
vi.mock('../state/useTaskPriority', () => ({ useTaskPriorityAction: () => ACTIONS.priority }));
vi.mock('../state/taskCascadeContext', () => ({ useTaskCascadeEnabled: () => CASCADE }));
vi.mock('./lastUsedTaskList', () => ({
  readLastUsedTaskList: () => null,
  writeLastUsedTaskList: () => {},
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<typeof import('../state/useTitleSuggestions')>(
    '../state/useTitleSuggestions',
  );
  return { ...actual, useTitleSuggestions: () => MATCHES.rows };
});
const MATCHES: { rows: Task[] } = { rows: [] };

/** Rendered after the editor: its effect runs right after the editor's. */
function StorePushesAfterTheEditor() {
  useEffect(() => {
    const current = store.get();
    store.set({
      ...current,
      selectedTaskListIds: new Set(current.selectedTaskListIds as Set<string>),
    });
  }, []);
  return null;
}

describe('TaskDialog prefill → list, a store pushing before the prefill renders', () => {
  it("keeps the offer's list", async () => {
    const { TaskDialog } = await import('./TaskDialog');
    render(
      <>
        <TaskDialog
          isOpen
          onClose={() => {}}
          task={null}
          defaultListId="list-inbox"
          defaultTitle="Einkaufen"
          prefillFrom={task('list-work')}
        />
        <StorePushesAfterTheEditor />
      </>,
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    const picker = screen.getByRole('combobox', { name: /liste/i }) as HTMLSelectElement;
    expect(picker.value).toBe('list-work');
  });
});
