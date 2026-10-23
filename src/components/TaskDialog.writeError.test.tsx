import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';

import type { Task, TaskList } from '../api/types';

/**
 * What the task editor says when a save fails (decision 273).
 *
 * A Tauri command rejects with a plain `{code, message}` object, and the
 * editor used to show it as `code: message` — a busy provider read out as
 * "forbidden: provider-busy: http-429", in English, in a German editor. Now a
 * refusal and a leftover read as their sentences, and a save that landed in
 * part only closes: saving again would write twice what already stands.
 */

const { invokeMock, answers, resetAnswers } = vi.hoisted(() => {
  /** What each command answers, by name: called in order, the last repeats. */
  const answers: Record<string, Array<(args: unknown) => Promise<unknown>>> = {};
  const calls: Record<string, number> = {};
  const invokeMock = vi.fn((command: string, args: unknown) => {
    const list = answers[command];
    if (!list || list.length === 0) {
      return Promise.resolve(command === 'task_current_user' ? null : []);
    }
    const n = calls[command] ?? 0;
    calls[command] = n + 1;
    return list[Math.min(n, list.length - 1)](args);
  });
  const resetAnswers = () => {
    for (const key of Object.keys(answers)) delete answers[key];
    for (const key of Object.keys(calls)) delete calls[key];
  };
  return { invokeMock, answers, resetAnswers };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const LIST: TaskList = {
  id: 'L1',
  name: 'Haushalt',
  read_only: false,
  task_capabilities: DEFAULT_TASK_CAPABILITIES,
} as unknown as TaskList;

const STORE = {
  taskLists: [LIST],
  selectedTaskListIds: new Set(['L1']),
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
const invalidateData = vi.fn();

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ invalidateData }),
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
  invalidateData.mockClear();
  resetAnswers();
});

const created = (id: string, title: string, parent: string | null = null): Task =>
  ({
    id,
    list_id: 'L1',
    title,
    status: 'open',
    parent_id: parent,
    section_id: null,
  }) as unknown as Task;

async function openNew(onClose = () => {}) {
  const { TaskDialog } = await import('./TaskDialog');
  render(<TaskDialog isOpen onClose={onClose} task={null} defaultListId="L1" />);
  fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
    target: { value: 'Einkaufen' },
  });
}

function addDraft(title: string) {
  fireEvent.change(screen.getByLabelText('Titel der neuen Unteraufgabe'), {
    target: { value: title },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Hinzufügen' }));
}

const create = () => fireEvent.click(screen.getByRole('button', { name: 'Anlegen' }));

const createCalls = () => invokeMock.mock.calls.filter((call) => call[0] === 'create_task');

describe('TaskDialog → a save that failed', () => {
  it('says a busy provider in words, never its token', async () => {
    answers.create_task = [
      () => Promise.reject({ code: 'forbidden', message: 'provider-busy: http-429' }),
    ];
    await openNew();
    create();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(
      'Der Anbieter ist gerade ausgelastet. Es wurde nichts geändert. Versuche es später noch einmal.',
    );
  });

  it('keeps what the host said for anything else', async () => {
    answers.create_task = [() => Promise.reject({ code: 'network', message: 'timed out' })];
    await openNew();
    create();
    expect((await screen.findByRole('alert')).textContent).toBe('network: timed out');
  });

  it('only closes once the task stands without one of its steps', async () => {
    answers.create_task = [
      () => Promise.reject({ code: 'network', message: 'partly-saved: assignees' }),
    ];
    const onClose = vi.fn();
    await openNew(onClose);
    addDraft('Milch');
    create();
    const note = await screen.findByText(/nur zum Teil gespeichert/);
    // Focused, so it is read out; the staged subtask was never made, and is
    // named so it is not lost without a word.
    await waitFor(() => expect(document.activeElement).toBe(note));
    expect(note.textContent).toContain('Die Unteraufgabe „Milch“ wurde nicht angelegt.');
    expect(screen.queryByRole('button', { name: 'Anlegen' })).toBeNull();
    expect(invalidateData).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Schließen' }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(createCalls()).toHaveLength(1);
  });

  it('stays on a created task whose subtasks did not all follow, and saves only those', async () => {
    answers.create_task = [
      () => Promise.resolve(created('t-1', 'Einkaufen')),
      () => Promise.resolve(created('t-2', 'Milch', 't-1')),
      () => Promise.reject({ code: 'forbidden', message: 'provider-busy: http-429' }),
      () => Promise.resolve(created('t-3', 'Brot', 't-1')),
    ];
    const onClose = vi.fn();
    await openNew(onClose);
    addDraft('Milch');
    addDraft('Brot');
    create();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(
      '„Einkaufen“ wurde angelegt, die Unteraufgabe aber nicht: der Anbieter ist gerade ausgelastet. Speichern legt nur sie an.',
    );
    expect(onClose).not.toHaveBeenCalled();
    // The draft that was made is gone from the form; the one that was not
    // stays.
    expect(screen.queryByText('Milch')).toBeNull();
    expect(screen.getByText('Brot')).toBeTruthy();

    create();
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    // The parent was not created again: only the draft left, under it.
    const sent = createCalls().map(
      (call) => (call[1] as { request: { title: string; parent_id: string | null } }).request,
    );
    expect(sent.map((r) => [r.title, r.parent_id])).toEqual([
      ['Einkaufen', null],
      ['Milch', 't-1'],
      ['Brot', 't-1'],
      ['Brot', 't-1'],
    ]);
  });
});
