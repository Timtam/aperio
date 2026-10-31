import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';

import type { Task, TaskList } from '../api/types';
import { CalendarStoreProvider } from '../state/CalendarStore';

/**
 * The task editor's inline section writes against the real store (decision
 * 277). A section made on a list whose sections could not be read is offered
 * and selected all the same: the picker says where the task is filed, as the
 * save sends it. A section write's failure is said at its field, and the next
 * one that works takes it away.
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
  task_capabilities: { ...DEFAULT_TASK_CAPABILITIES, sections: true, manageable_sections: true },
} as unknown as TaskList;

/** A second list, to move to. */
const OTHER = { ...LIST, id: 'L2', name: 'Arbeit' } as TaskList;

const VIEW_STATE = { showHiddenTaskListTargets: false, timeStepMinutes: 15 };
const CASCADE = {
  enabled: false,
  autoDate: false,
  autoSelfAssign: false,
  priorityScale: 'three',
};
const announce = vi.fn();

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
  return { ...actual, useTitleSuggestions: () => [] };
});

const BUSY = { code: 'forbidden', message: 'provider-busy: http-429' };
const SAID_BUSY =
  'Der Anbieter ist gerade ausgelastet. Es wurde nichts geändert. Versuche es später noch einmal.';
const EINKAUF = { id: 's1', list_id: 'L1', name: 'Einkauf', position: 0, color_label: null };
/** A local write's failure: a section's colour never reaches a provider. */
const LOCKED = { code: 'internal', message: 'database is locked' };

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  announce.mockClear();
  resetAnswers();
});

async function openNew() {
  answers.list_task_lists = [() => Promise.resolve([LIST])];
  const { TaskDialog } = await import('./TaskDialog');
  render(
    <CalendarStoreProvider>
      <TaskDialog isOpen onClose={() => {}} task={null} defaultListId="L1" />
    </CalendarStoreProvider>,
  );
  return (await screen.findByLabelText('Abschnitt')) as HTMLSelectElement;
}

async function addSection(name: string) {
  fireEvent.click(screen.getByRole('button', { name: '+ Abschnitt' }));
  const box = screen.getByLabelText('Name des neuen Abschnitts');
  fireEvent.change(box, { target: { value: name } });
  const editor = box.parentElement as HTMLElement;
  fireEvent.click(within(editor).getByRole('button', { name: 'Anlegen' }));
}

describe('TaskDialog → a section written where none could be read', () => {
  it('offers and selects the new section when every read of the list failed', async () => {
    // Todoist turns the first read away and the one after the create too:
    // the create landed, so the picker shows "Einkauf", as the save sends it.
    answers.get_sections = [() => Promise.reject(BUSY)];
    answers.create_section = [() => Promise.resolve(EINKAUF)];
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const picker = await openNew();
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([cmd]) => cmd === 'get_sections')).toBe(true),
    );
    await addSection('Einkauf');
    await waitFor(() => expect(picker.value).toBe('s1'));
    expect(picker.selectedOptions[0]?.textContent).toBe('Einkauf');
    expect(announce).toHaveBeenCalledWith('Abschnitt „Einkauf" wurde angelegt.');
    warn.mockRestore();
  });

  it("says the sections could not be read, and keeps the task's own section", async () => {
    // A task filed in "Einkauf", its list's sections turned away: the picker
    // must not read "No section" while the save keeps the section.
    answers.list_task_lists = [() => Promise.resolve([LIST, OTHER])];
    answers.get_sections = [() => Promise.reject(BUSY)];
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const task = {
      id: 't1',
      list_id: 'L1',
      title: 'Milch',
      status: 'open',
      priority: 'medium',
      effort: 'medium',
      assignees: [],
      reminders: [],
      parent_id: null,
      section_id: 's1',
    } as unknown as Task;
    const { TaskDialog } = await import('./TaskDialog');
    render(
      <CalendarStoreProvider>
        <TaskDialog isOpen onClose={() => {}} task={task} />
      </CalendarStoreProvider>,
    );
    const picker = (await screen.findByLabelText('Abschnitt')) as HTMLSelectElement;
    await waitFor(() =>
      expect(picker).toHaveAccessibleDescription(
        'Die Abschnitte dieser Liste konnten nicht geladen werden: der Anbieter ist gerade ' +
          'ausgelastet. Versuche es später noch einmal.',
      ),
    );
    expect(picker.value).toBe('s1');
    expect(picker.selectedOptions[0]?.textContent).toBe('Bisheriger Abschnitt (nicht geladen)');
    // Its name is not known: it is neither renamed nor deleted from here.
    const field = within(picker.closest('.form__field') as HTMLElement);
    expect(field.getByRole('button', { name: '+ Abschnitt' })).toBeTruthy();
    expect(field.queryByRole('button', { name: 'Umbenennen' })).toBeNull();
    expect(field.queryByRole('button', { name: 'Löschen' })).toBeNull();
    // In another list it is no place at all.
    const lists = screen.getByLabelText('Liste') as HTMLSelectElement;
    await waitFor(() => expect(lists.querySelector('option[value="L2"]')).not.toBeNull());
    fireEvent.change(lists, { target: { value: 'L2' } });
    await waitFor(() => expect(picker.querySelector('option[value="s1"]')).toBeNull());
    expect(screen.queryByText('Bisheriger Abschnitt (nicht geladen)')).toBeNull();
    warn.mockRestore();
  });

  it('carries no section to another list, even one whose sections could not be read', async () => {
    // "Einkauf" was picked in "Haushalt"; the list then changes to one whose
    // sections are turned away. The old section is no place in it.
    answers.list_task_lists = [() => Promise.resolve([LIST, OTHER])];
    answers.get_sections = [
      (args) =>
        (args as { listId: string }).listId === 'L1'
          ? Promise.resolve([EINKAUF])
          : Promise.reject(BUSY),
    ];
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { TaskDialog } = await import('./TaskDialog');
    render(
      <CalendarStoreProvider>
        <TaskDialog isOpen onClose={() => {}} task={null} defaultListId="L1" />
      </CalendarStoreProvider>,
    );
    const picker = (await screen.findByLabelText('Abschnitt')) as HTMLSelectElement;
    await waitFor(() => expect(picker.querySelector('option[value="s1"]')).not.toBeNull());
    fireEvent.change(picker, { target: { value: 's1' } });
    const lists = screen.getByLabelText('Liste') as HTMLSelectElement;
    await waitFor(() => expect(lists.querySelector('option[value="L2"]')).not.toBeNull());
    fireEvent.change(lists, { target: { value: 'L2' } });
    await waitFor(() => expect(picker).toHaveAccessibleDescription(/nicht geladen werden/));
    expect(picker.value).toBe('');
    expect(screen.queryByText('Bisheriger Abschnitt (nicht geladen)')).toBeNull();
    // And the save sends what the picker says: no section.
    answers.create_task = [(args) => Promise.resolve({ ...(args as { request: object }).request, id: 't9' })];
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$/i }), {
      target: { value: 'Milch' },
    });
    const dialogForm = within(picker.closest('form') as HTMLElement);
    fireEvent.click(dialogForm.getAllByRole('button', { name: 'Anlegen' }).at(-1) as HTMLElement);
    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([cmd]) => cmd === 'create_task')).toBe(true),
    );
    const sent = invokeMock.mock.calls.find(([cmd]) => cmd === 'create_task')?.[1] as {
      request: { list_id: string; section_id: string | null };
    };
    expect(sent.request).toMatchObject({ list_id: 'L2', section_id: null });
    warn.mockRestore();
  });

  it('takes a section failure away once the next try lands', async () => {
    answers.get_sections = [() => Promise.resolve([]), () => Promise.resolve([EINKAUF])];
    answers.create_section = [() => Promise.reject(BUSY), () => Promise.resolve(EINKAUF)];
    const picker = await openNew();
    await addSection('Einkauf');
    expect((await screen.findByRole('alert')).textContent).toBe(SAID_BUSY);
    const editor = screen.getByLabelText('Name des neuen Abschnitts').parentElement as HTMLElement;
    fireEvent.click(within(editor).getByRole('button', { name: 'Anlegen' }));
    await waitFor(() => expect(picker.value).toBe('s1'));
    expect(screen.queryByText(SAID_BUSY)).toBeNull();
  });

  it('never says "nothing was changed" when the colour fails after the section landed', async () => {
    // The section stands; only its colour — a local write, no provider's —
    // did not follow. "Nothing was changed" would be false, and creating it
    // again would make two.
    answers.list_color_labels = [
      () => Promise.resolve([{ id: 'red', name: 'Rot', color: '#ff0000', position: 0 }]),
    ];
    answers.get_sections = [() => Promise.resolve([]), () => Promise.resolve([EINKAUF])];
    answers.create_section = [() => Promise.resolve(EINKAUF)];
    answers.set_section_color = [() => Promise.reject(LOCKED)];
    const picker = await openNew();
    fireEvent.click(screen.getByRole('button', { name: '+ Abschnitt' }));
    fireEvent.change(screen.getByLabelText('Name des neuen Abschnitts'), {
      target: { value: 'Einkauf' },
    });
    const color = (await screen.findByLabelText('Abschnittsfarbe')) as HTMLSelectElement;
    await waitFor(() => expect(color.querySelector('option[value="red"]')).not.toBeNull());
    fireEvent.change(color, { target: { value: 'red' } });
    const editor = screen.getByLabelText('Name des neuen Abschnitts').parentElement as HTMLElement;
    fireEvent.click(within(editor).getByRole('button', { name: 'Anlegen' }));
    await waitFor(() => expect(picker.value).toBe('s1'));
    expect((await screen.findByRole('alert')).textContent).toBe(
      'network: section-color: internal error: database is locked',
    );
    expect(screen.queryByLabelText('Name des neuen Abschnitts')).toBeNull();
  });
});
