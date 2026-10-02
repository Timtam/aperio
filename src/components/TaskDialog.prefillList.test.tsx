import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';

import type { Task, TaskList } from '../api/types';

/**
 * The task editor filled from an offer whose list it cannot use (decision
 * 161): it keeps its own list, shows why under the picker, and says the fill
 * and the reason as one sentence, once.
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

const STATE: { taskLists: TaskList[] } = { taskLists: [INBOX, WORK, SHARED] };
const STORE = {
  get taskLists() {
    return STATE.taskLists;
  },
  selectedTaskListIds: new Set(['list-inbox', 'list-work', 'list-shared']),
  colorLabels: [],
  sectionsByList: {},
  loadSections: () => Promise.resolve(),
};
const VIEW_STATE = { showHiddenTaskListTargets: false, timeStepMinutes: 15 };
const CASCADE = { enabled: false, autoDate: false, autoSelfAssign: false, priorityScale: 'three' };
const announce = vi.hoisted(() => vi.fn());

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ invalidateData: () => {} }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../state/useTasks', () => ({ useTasks: () => ({ tasks: [] }) }));
vi.mock('../state/useTaskStatusToggle', () => ({
  useTaskStatusActions: () => ({ toggle: () => {}, set: () => {} }),
}));
vi.mock('../state/useTaskPriority', () => ({ useTaskPriorityAction: () => () => {} }));
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

afterEach(() => {
  document.body.innerHTML = '';
  announce.mockClear();
  MATCHES.rows = [];
  STATE.taskLists = [INBOX, WORK, SHARED];
});

const REFUSAL =
  'Aus „Einkaufen" übernommen. Der Tag bleibt, wie er war. ' +
  '„Geteilt“ nimmt keine neuen Aufgaben an. Die Aufgabe kommt in „Eingang“.';

async function open(prefillFrom?: Task) {
  const { TaskDialog } = await import('./TaskDialog');
  const element = () => (
    <TaskDialog
      isOpen
      onClose={() => {}}
      task={null}
      defaultListId="list-inbox"
      defaultTitle="Einkaufen"
      prefillFrom={prefillFrom}
    />
  );
  const view = render(element());
  await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
  const picker = screen.getByRole('combobox', { name: /liste/i }) as HTMLSelectElement;
  return { picker, rerender: () => view.rerender(element()) };
}

describe('TaskDialog filled from an offer', () => {
  it("takes the offer's list when it takes new tasks", async () => {
    const { picker } = await open(task('list-work'));
    expect(picker.value).toBe('list-work');
    expect(announce).not.toHaveBeenCalled();
  });

  it('keeps its own list for a read-only one, and says so once, with the fill', async () => {
    const { picker, rerender } = await open(task('list-shared'));
    expect(picker.value).toBe('list-inbox');
    expect(announce.mock.calls.map((c) => c[0])).toEqual([REFUSAL]);
    expect(screen.getByText(/„Geteilt“ nimmt keine neuen Aufgaben an/)).toBeTruthy();

    // A catalog refresh re-derives the untouched form: the prefill lands
    // again, but its sentence is not said again, and the note stays.
    STATE.taskLists = [...STATE.taskLists];
    rerender();
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(announce.mock.calls.map((c) => c[0])).toEqual([REFUSAL]);
    expect(screen.getByText(/„Geteilt“ nimmt keine neuen Aufgaben an/)).toBeTruthy();
  });

  it('waits for the list catalog instead of refusing every list as unknown', async () => {
    STATE.taskLists = [];
    const { rerender } = await open(task('list-work'));
    STATE.taskLists = [INBOX, WORK, SHARED];
    rerender();
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    const picker = screen.getByRole('combobox', { name: /liste/i }) as HTMLSelectElement;
    expect(picker.value).toBe('list-work');
    expect(announce).not.toHaveBeenCalled();
  });

  it('keeps the note of an offer taken in the editor through a catalog refresh', async () => {
    // Taken in the editor's own title field, the read-only offer leaves the
    // form as it was, so a refresh resets it as untouched — and nothing
    // re-applies a prefill here to bring a wiped note back.
    MATCHES.rows = [task('list-shared')];
    const { rerender } = await open();
    const title = screen.getByRole('combobox', { name: /titel/i });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'Enter' });
    expect(screen.getByText(/„Geteilt“ nimmt keine neuen Aufgaben an/)).toBeTruthy();
    STATE.taskLists = [...STATE.taskLists];
    rerender();
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(screen.getByText(/„Geteilt“ nimmt keine neuen Aufgaben an/)).toBeTruthy();
  });
});
