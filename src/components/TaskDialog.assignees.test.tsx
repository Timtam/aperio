import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';

import type { TaskList } from '../api/types';

/**
 * The "Assigned to" field says what it knows (decision 130).
 *
 * The people a task can be assigned to are read from the provider whenever
 * the editor opens a list. A failed read used to be an empty list, and an
 * empty list hid the field: a Vikunja token made before 2.4 lacks the
 * permission that read needs, and every picker vanished without a word. Now
 * the field stays wherever the list can hold assignees, and says whether the
 * people are loading, could not be read — and why — or are nobody.
 */

const { invokeMock, members } = vi.hoisted(() => {
  /** What `task_list_members` answers, one entry per call; the last repeats. */
  const members: { answers: Array<() => Promise<unknown>> } = { answers: [] };
  let calls = 0;
  const invokeMock = vi.fn((command: string) => {
    if (command === 'task_list_members') {
      const answer = members.answers[Math.min(calls, members.answers.length - 1)];
      calls += 1;
      return answer ? answer() : Promise.resolve([]);
    }
    if (command === 'task_current_user') return Promise.resolve(null);
    return Promise.resolve([]);
  });
  const reset = () => {
    calls = 0;
  };
  return { invokeMock, members: Object.assign(members, { reset }) };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

/** A shared Vikunja list: it holds any number of assignees. */
const SHARED: TaskList = {
  id: 'list-shared',
  name: 'Haushalt',
  read_only: false,
  task_capabilities: { ...DEFAULT_TASK_CAPABILITIES, task_assignment: 'multiple' },
} as unknown as TaskList;
/** A local list: nobody to assign, ever. */
const LOCAL: TaskList = {
  id: 'list-local',
  name: 'Lokal',
  read_only: false,
  task_capabilities: DEFAULT_TASK_CAPABILITIES,
} as unknown as TaskList;

const STORE = {
  taskLists: [SHARED, LOCAL],
  selectedTaskListIds: new Set(['list-shared', 'list-local']),
  colorLabels: [],
  sectionsByList: {},
  loadSections: () => Promise.resolve(),
};
const VIEW_STATE = { showHiddenTaskListTargets: false, timeStepMinutes: 15 };
const CASCADE = {
  enabled: false,
  autoDate: false,
  autoSelfAssign: false,
  priorityScale: 'three',
};

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ invalidateData: () => {} }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
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
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  members.answers = [];
  members.reset();
});

async function open(listId: string) {
  const { TaskDialog } = await import('./TaskDialog');
  render(<TaskDialog isOpen onClose={() => {}} task={null} defaultListId={listId} />);
}

const poolReads = () =>
  invokeMock.mock.calls.filter((call) => call[0] === 'task_list_members').length;

describe('TaskDialog → "Assigned to"', () => {
  it('names the permission a refused token lacks, and reads again on request', async () => {
    members.answers = [
      () =>
        Promise.reject({
          code: 'forbidden',
          message: 'token-refused: users search (projects)',
        }),
      () => Promise.resolve([{ id: '3', name: 'bob' }]),
    ];
    await open('list-shared');

    const failure = await screen.findByText(/users search \(projects\)/);
    // The refusal's own sentence, not the general "could not be loaded" with
    // the raw message in it.
    expect(failure.textContent).toMatch(
      /API-Token des Kontos abgelehnt|refused the account's API token/,
    );
    expect(failure.textContent).not.toMatch(/token-refused/);
    const retry = screen.getByRole('button', { name: /erneut versuchen|try again/i });
    expect(retry.getAttribute('aria-describedby')).toBe(failure.id);

    // The button unmounts the moment it is pressed; focus must not fall out
    // of the field, and ends on the picker once the people arrive.
    retry.focus();
    fireEvent.click(retry);
    await waitFor(() => expect(poolReads()).toBe(2));
    await waitFor(() =>
      expect(screen.queryByText(/users search \(projects\)/)).toBeNull(),
    );
    expect(screen.queryByRole('button', { name: /erneut versuchen|try again/i })).toBeNull();
    const field = screen.getByRole('group', { name: /zugewiesen an|assigned to/i });
    await waitFor(() => expect(field.contains(document.activeElement)).toBe(true));
    expect(document.activeElement?.tagName).toBe('SELECT');
    // The select says which field it is, not only its first option.
    expect(screen.getByRole('combobox', { name: /zugewiesen an|assigned to/i })).toBe(
      document.activeElement,
    );
  });

  it('says any other failure too, with what the host said', async () => {
    members.answers = [() => Promise.reject({ code: 'network', message: 'offline' })];
    await open('list-shared');
    await screen.findByText(/konnten nicht geladen werden: network: offline|could not be loaded: network: offline/);
  });

  it('says so when nobody on the list can be assigned', async () => {
    members.answers = [() => Promise.resolve([])];
    await open('list-shared');
    await screen.findByText(/niemanden zum Zuweisen|Nobody on this list/);
  });

  it('shows no field where the list cannot hold assignees at all', async () => {
    await open('list-local');
    await waitFor(() => expect(poolReads()).toBe(1));
    expect(screen.queryByText(/^Zugewiesen an$|^Assigned to$/)).toBeNull();
  });
});
