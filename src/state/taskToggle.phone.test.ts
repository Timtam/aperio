import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { StatusWrite, Task } from '@aperio/shared';

/**
 * The phone's status cascade (`applyStatusWrites`, the twin of the desktop's
 * `applyCascade`): every write is attempted, and where another write of the
 * cascade landed or may have, the failure it throws never says "nothing was
 * changed" (decision 277). The phone has no test runner of its own, so its module runs
 * here.
 */

const { updateTask } = vi.hoisted(() => ({ updateTask: vi.fn() }));
vi.mock('../../mobile/src/api/client', () => ({ updateTask }));
vi.mock('../../mobile/src/state/currentUser', () => ({
  currentUserForList: async () => null,
}));
vi.mock('../../mobile/src/state/taskBehaviour', () => ({
  canStoreInProgress: () => true,
  effectiveForList: () => ({ cascade: true, autoDate: false }),
  nextCheckoffStatus: () => 'completed',
  readTaskBehaviour: async () => ({ autoSelfAssign: false }),
}));

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_TASK_TOGGLE = '../../mobile/src/state/taskToggle';
type ApplyStatusWrites = (
  writes: StatusWrite[],
  snapshot: Task[],
  autoSelfAssign: boolean,
) => Promise<void>;
async function applyStatusWrites(...args: Parameters<ApplyStatusWrites>): Promise<void> {
  const phoneModule = (await import(/* @vite-ignore */ PHONE_TASK_TOGGLE)) as {
    applyStatusWrites: ApplyStatusWrites;
  };
  return phoneModule.applyStatusWrites(...args);
}

const task = (id: string) =>
  ({ id, list_id: 'L1', title: id, status: 'open', assignees: [] }) as unknown as Task;
const SNAPSHOT = [task('root'), task('kid')];
const WRITES = [
  { taskId: 'root', status: 'completed' },
  { taskId: 'kid', status: 'completed' },
] as StatusWrite[];
const BUSY = { code: 'forbidden', message: 'provider-busy: http-429' };

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  updateTask.mockReset();
});

describe('the phone status cascade (277)', () => {
  it('reads a busy write as partly saved once another write landed', async () => {
    updateTask.mockRejectedValueOnce(BUSY).mockResolvedValueOnce({});
    await expect(applyStatusWrites(WRITES, SNAPSHOT, false)).rejects.toEqual({
      code: 'network',
      message: 'partly-saved: cascade',
    });
    expect(updateTask).toHaveBeenCalledTimes(2);
  });

  it('keeps the busy refusal when nothing landed', async () => {
    updateTask.mockRejectedValue(BUSY);
    await expect(applyStatusWrites(WRITES, SNAPSHOT, false)).rejects.toEqual(BUSY);
  });

  it('reads a busy write as partly saved when another write may have landed', async () => {
    // The second write's answer is lost: it may stand.
    updateTask
      .mockRejectedValueOnce(BUSY)
      .mockRejectedValueOnce({ code: 'network', message: 'connection reset' });
    await expect(applyStatusWrites(WRITES, SNAPSHOT, false)).rejects.toEqual({
      code: 'network',
      message: 'partly-saved: cascade',
    });
  });
});

describe('the family after a root saved in part with its status', () => {
  it('tries every write and hands back each failure as it came', async () => {
    // Review round 18 of #128: wrapped as the cascade's, its reason repeated
    // the root's sentence, unlike the desktop's.
    const kid = (id: string) =>
      ({ ...task(id), parent_id: 'root' }) as unknown as Task;
    const family = [{ ...task('root'), status: 'completed' } as Task, kid('k1'), kid('k2')];
    updateTask.mockRejectedValueOnce(BUSY).mockResolvedValueOnce({});
    const phoneModule = (await import(/* @vite-ignore */ PHONE_TASK_TOGGLE)) as {
      followEditorStatus: (
        taskId: string,
        status: string,
        listId: string,
        list: undefined,
        snapshot: Task[],
      ) => Promise<unknown[]>;
    };
    const failures = await phoneModule.followEditorStatus(
      'root',
      'completed',
      'L1',
      undefined,
      family,
    );
    expect(failures).toEqual([BUSY]);
    expect(updateTask).toHaveBeenCalledTimes(2);
  });
});

describe('the phone check-off of a task saved in part with its status', () => {
  it('says the task, then what of its family did not take', async () => {
    // Review round 20 of #128: the family's failure was dropped, unlike on
    // the desktop.
    const kid = { ...task('kid'), parent_id: 'root' } as unknown as Task;
    updateTask
      .mockRejectedValueOnce({ code: 'network', message: 'partly-saved: assignees' })
      .mockRejectedValueOnce(BUSY);
    const phoneModule = (await import(/* @vite-ignore */ PHONE_TASK_TOGGLE)) as {
      setTaskStatusTo: (task: Task, status: string, list: undefined, all: Task[]) => Promise<number>;
      checkOffErrorMessage: (err: unknown, t: (key: string) => string) => string;
    };
    const failure = await phoneModule
      .setTaskStatusTo(task('root'), 'completed', undefined, [task('root'), kid])
      .then(() => null, (err: unknown) => err);
    const said = phoneModule.checkOffErrorMessage(failure, (key) => key);
    expect(said).toContain('dialogs.event.writeError.partlySaved');
    expect(said).toContain('dialogs.task.subtasks.familyNotAll');
  });

  it('says nothing of a family member whose save stood in part with its status', async () => {
    // Review round 21 of #128: it took the new status, yet the sentence said
    // it did not.
    const kid = { ...task('kid'), parent_id: 'root' } as unknown as Task;
    updateTask
      .mockRejectedValueOnce({ code: 'network', message: 'partly-saved: assignees' })
      .mockRejectedValueOnce({ code: 'network', message: 'partly-saved: assignees' });
    const phoneModule = (await import(/* @vite-ignore */ PHONE_TASK_TOGGLE)) as {
      setTaskStatusTo: (task: Task, status: string, list: undefined, all: Task[]) => Promise<number>;
      checkOffErrorMessage: (err: unknown, t: (key: string) => string) => string;
    };
    const failure = await phoneModule
      .setTaskStatusTo(task('root'), 'completed', undefined, [task('root'), kid])
      .then(() => null, (err: unknown) => err);
    const said = phoneModule.checkOffErrorMessage(failure, (key) => key);
    expect(said).toContain('dialogs.event.writeError.partlySaved');
    expect(said).not.toContain('dialogs.task.subtasks.familyNotAll');
  });
});

describe('the phone editor cascade after a list change', () => {
  it('writes each moved subtask as the move returned it', async () => {
    // Review round 20 of #128: written under the id the move deleted, it
    // failed on every external list.
    const kid = { ...task('kid'), parent_id: 'root' } as unknown as Task;
    const movedKid = { ...kid, id: 'kid-new', list_id: 'L2' } as unknown as Task;
    updateTask.mockResolvedValue({});
    const phoneModule = (await import(/* @vite-ignore */ PHONE_TASK_TOGGLE)) as {
      cascadeEditorStatus: (
        taskId: string,
        status: string,
        listId: string,
        list: undefined,
        snapshot: Task[],
        since: Map<string, Task>,
      ) => Promise<void>;
    };
    await phoneModule.cascadeEditorStatus(
      'root',
      'completed',
      'L2',
      undefined,
      [{ ...task('root'), status: 'completed' } as Task, kid],
      new Map([['kid', movedKid]]),
    );
    expect(updateTask).toHaveBeenCalledTimes(1);
    expect(updateTask.mock.calls[0][0]).toMatchObject({ id: 'kid-new', list_id: 'L2' });
  });
});
