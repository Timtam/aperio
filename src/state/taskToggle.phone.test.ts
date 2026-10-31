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
