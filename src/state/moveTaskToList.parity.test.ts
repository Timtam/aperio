import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Task } from '../api/types';

/**
 * A task moved with its subtasks, on both surfaces (decision 277): once the
 * task itself moved, a subtask the provider turns away never reads as
 * "nothing was changed". A busy provider leaves the move partly saved; any
 * other refusal becomes an error that may have landed. The phone has no test
 * runner of its own, so its module runs here, next to the desktop's.
 */

const { invokeMock, phoneUpdateTask } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  phoneUpdateTask: vi.fn(),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../../mobile/src/api/calendar', () => ({}));
vi.mock('../../mobile/src/api/client', () => ({
  createTask: vi.fn(),
  updateTask: phoneUpdateTask,
}));

import { moveTaskToList as desktopMoveTaskToList } from './moveActions';

/** The phone's module, through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_MOVE_ACTIONS = '../../mobile/src/state/moveActions';
type MoveTaskToList = (
  task: Task,
  targetListId: string,
  children: Task[],
  sectionId: string | null,
) => Promise<Task[]>;
async function phoneMoveTaskToList(...args: Parameters<MoveTaskToList>): Promise<Task[]> {
  const phoneModule = (await import(/* @vite-ignore */ PHONE_MOVE_ACTIONS)) as {
    moveTaskToList: MoveTaskToList;
  };
  return phoneModule.moveTaskToList(...args);
}

const task = (id: string, parent: string | null = null) =>
  ({ id, list_id: 'L1', title: id, parent_id: parent, section_id: null }) as unknown as Task;

/** Both surfaces' write, answering the parent and then failing the child. */
function answer(childFails: unknown) {
  const reply = (moved: Task) =>
    moved.parent_id == null
      ? Promise.resolve({ ...moved, id: 'p-new' })
      : Promise.reject(childFails);
  invokeMock.mockImplementation((_cmd: string, args: { task: Task }) => reply(args.task));
  phoneUpdateTask.mockImplementation((moved: Task) => reply(moved));
}

const surfaces = [
  ['desktop', desktopMoveTaskToList],
  ['phone', phoneMoveTaskToList],
] as const;

beforeEach(() => {
  invokeMock.mockReset();
  phoneUpdateTask.mockReset();
});

describe('moveTaskToList: a subtask turned away after its parent moved (277)', () => {
  it.each(surfaces)('%s: a busy provider leaves the move partly saved', async (_name, move) => {
    answer({ code: 'forbidden', message: 'provider-busy: http-429' });
    await expect(move(task('p'), 'L2', [task('c', 'p')], null)).rejects.toEqual({
      code: 'network',
      message: 'partly-saved: subtasks',
    });
  });

  it.each(surfaces)('%s: a refusal reads as may have landed', async (_name, move) => {
    answer({ code: 'not_found', message: 'gone' });
    await expect(move(task('p'), 'L2', [task('c', 'p')], null)).rejects.toEqual({
      code: 'network',
      message: 'subtasks: resource not found: gone',
    });
  });

  it.each(surfaces)('%s: the parent itself keeps its own error', async (_name, move) => {
    invokeMock.mockRejectedValue({ code: 'forbidden', message: 'provider-busy: http-429' });
    phoneUpdateTask.mockRejectedValue({ code: 'forbidden', message: 'provider-busy: http-429' });
    await expect(move(task('p'), 'L2', [task('c', 'p')], null)).rejects.toEqual({
      code: 'forbidden',
      message: 'provider-busy: http-429',
    });
  });
});
