import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Task, TaskList } from '../api/types';

/**
 * The task twin of `QuickAddDialog.suggest.test.tsx`.
 *
 * Accepting an offer hands the editor the earlier task AND says whether its
 * list may travel. `targetPinned` means "the user picked a list HERE, don't
 * overrule it" — a fact about what the user did, which is why it is recorded
 * rather than inferred from a default that moves on its own.
 */

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const LISTS: TaskList[] = [
  { id: 'list-inbox', name: 'Eingang', read_only: false } as unknown as TaskList,
  { id: 'list-work', name: 'Arbeit', read_only: false } as unknown as TaskList,
];

const SOURCE: Task = {
  id: 'task-thomas',
  list_id: 'list-work',
  title: 'Thomas anrufen',
  description: null,
  due: null,
  completed: false,
} as unknown as Task;

const STORE = {
  taskLists: LISTS as TaskList[],
  selectedTaskListIds: new Set(['list-inbox', 'list-work']),
};
const VIEW_STATE = { showHiddenTaskListTargets: false };

const openTaskDialog = vi.hoisted(() => vi.fn());

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ openTaskDialog }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('./lastUsedTaskList', () => ({
  readLastUsedTaskList: () => null,
  writeLastUsedTaskList: () => {},
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [SOURCE] };
});

afterEach(() => {
  document.body.innerHTML = '';
  openTaskDialog.mockClear();
  STORE.taskLists = LISTS;
  STORE.selectedTaskListIds = new Set(['list-inbox', 'list-work']);
});

function acceptTheOffer() {
  const title = screen.getByRole('combobox', { name: /titel/i });
  fireEvent.change(title, { target: { value: 'thomas' } });
  fireEvent.keyDown(title, { key: 'ArrowDown' });
  fireEvent.keyDown(title, { key: 'Enter' });
}

describe('QuickAddTaskDialog → accepting a title offer', () => {
  it('does NOT pin the list when the user never picked one', async () => {
    const { QuickAddTaskDialog } = await import('./QuickAddTaskDialog');
    render(<QuickAddTaskDialog isOpen onClose={() => {}} />);
    acceptTheOffer();
    await waitFor(() => expect(openTaskDialog).toHaveBeenCalled());
    const opts = openTaskDialog.mock.calls[0][1] as {
      prefillFrom?: Task;
      targetPinned?: boolean;
    };
    expect(opts.prefillFrom?.id).toBe('task-thomas');
    expect(opts.targetPinned).toBe(false);
  });

  it("keeps the user's pick even after the default catches up with it", async () => {
    // The list the user chose was silently dropped as soon as the default
    // moved onto it — a background catalog refresh, a changed selection. The
    // comparison then read "untouched" and the offer's list took over.
    const { QuickAddTaskDialog } = await import('./QuickAddTaskDialog');
    const { rerender } = render(
      <QuickAddTaskDialog isOpen onClose={() => {}} />,
    );
    const picker = screen.getByRole('combobox', { name: /liste/i });
    fireEvent.change(picker, { target: { value: 'list-work' } });

    STORE.selectedTaskListIds = new Set(['list-work']);
    rerender(<QuickAddTaskDialog isOpen onClose={() => {}} />);

    acceptTheOffer();
    await waitFor(() => expect(openTaskDialog).toHaveBeenCalled());
    const opts = openTaskDialog.mock.calls[0][1] as { targetPinned?: boolean };
    expect(opts.targetPinned).toBe(true);
  });

  it('does not pin when the DEFAULT moves under an untouched picker', async () => {
    const { QuickAddTaskDialog } = await import('./QuickAddTaskDialog');
    const { rerender } = render(
      <QuickAddTaskDialog isOpen onClose={() => {}} />,
    );
    const picker = screen.getByRole('combobox', {
      name: /liste/i,
    }) as HTMLSelectElement;
    await waitFor(() => expect(picker.value).toBe('list-inbox'));

    STORE.selectedTaskListIds = new Set(['list-work']);
    rerender(<QuickAddTaskDialog isOpen onClose={() => {}} />);
    await waitFor(() => expect(picker.value).toBe('list-work'));

    acceptTheOffer();
    await waitFor(() => expect(openTaskDialog).toHaveBeenCalled());
    const opts = openTaskDialog.mock.calls[0][1] as { targetPinned?: boolean };
    expect(opts.targetPinned).toBe(false);
  });
});
