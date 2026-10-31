import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import { DEFAULT_TASK_CAPABILITIES } from '@aperio/shared';

import type { Task, TaskList } from '../api/types';
import { CalendarStoreProvider } from '../state/CalendarStore';

/**
 * Move/Copy's "new section" against the real store (decision 277): a section
 * made in a list whose sections could not be read is offered and selected,
 * the picker says why the rest is missing, and a failed try is said at the
 * field until the next one lands.
 */

const { invokeMock, answers, resetAnswers } = vi.hoisted(() => {
  /** What each command answers, by name: called in order, the last repeats. */
  const answers: Record<string, Array<(args: unknown) => Promise<unknown>>> = {};
  const calls: Record<string, number> = {};
  const invokeMock = vi.fn((command: string, args: unknown) => {
    const list = answers[command];
    if (!list || list.length === 0) return Promise.resolve([]);
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

const announce = vi.fn();
vi.mock('../state/viewStateContext', () => ({
  useViewState: () => ({ showHiddenCalendarTargets: false, showHiddenTaskListTargets: false }),
}));
vi.mock('../state/useTasks', () => ({ useTasks: () => ({ tasks: [] }) }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ invalidateData: () => {} }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));

const BUSY = { code: 'forbidden', message: 'provider-busy: http-429' };
const SAID_BUSY =
  'Der Anbieter ist gerade ausgelastet. Es wurde nichts geändert. Versuche es später noch einmal.';
const EINKAUF = { id: 's1', list_id: 'L1', name: 'Einkauf', position: 0, color_label: null };
const TASK = {
  id: 't1',
  list_id: 'L1',
  title: 'Milch',
  status: 'open',
  parent_id: null,
  section_id: null,
} as unknown as Task;

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  announce.mockClear();
  resetAnswers();
});

async function open() {
  answers.list_task_lists = [() => Promise.resolve([LIST])];
  const { MoveCopyDialog } = await import('./MoveCopyDialog');
  render(
    <div id="app-root">
      <CalendarStoreProvider>
        <MoveCopyDialog isOpen onClose={() => {}} target={{ kind: 'task', task: TASK }} />
      </CalendarStoreProvider>
    </div>,
  );
  return (await screen.findByLabelText('Abschnitt')) as HTMLSelectElement;
}

function create(name: string) {
  if (!screen.queryByLabelText('Name')) {
    fireEvent.click(screen.getByRole('button', { name: '+ Abschnitt' }));
  }
  const box = screen.getByLabelText('Name');
  fireEvent.change(box, { target: { value: name } });
  const editor = box.parentElement as HTMLElement;
  fireEvent.click(within(editor).getByRole('button', { name: 'Anlegen' }));
}

describe('MoveCopyDialog → a new section where none could be read', () => {
  it('offers and selects it, and says why the rest is missing', async () => {
    answers.get_sections = [() => Promise.reject(BUSY)];
    answers.create_section = [() => Promise.resolve(EINKAUF)];
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const picker = await open();
    await waitFor(() =>
      expect(picker).toHaveAccessibleDescription(
        'Die Abschnitte dieser Liste konnten nicht geladen werden: der Anbieter ist gerade ' +
          'ausgelastet. Versuche es später noch einmal.',
      ),
    );
    create('Einkauf');
    await waitFor(() => expect(picker.value).toBe('s1'));
    expect(picker.selectedOptions[0]?.textContent).toBe('Einkauf');
    warn.mockRestore();
  });

  it('takes a failed try away once the next one lands', async () => {
    answers.get_sections = [() => Promise.resolve([]), () => Promise.resolve([EINKAUF])];
    answers.create_section = [() => Promise.reject(BUSY), () => Promise.resolve(EINKAUF)];
    const picker = await open();
    create('Einkauf');
    expect((await screen.findByRole('alert')).textContent).toBe(SAID_BUSY);
    create('Einkauf');
    await waitFor(() => expect(picker.value).toBe('s1'));
    expect(screen.queryByText(SAID_BUSY)).toBeNull();
  });
});
