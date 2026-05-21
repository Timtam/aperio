import { describe, expect, it } from 'vitest';

import type { Task, TaskStatus } from '../api/types';
import {
  deriveStatusFromChildren,
  planAncestorRecompute,
  planStatusCascade,
} from './taskCascade';

const baseTask: Task = {
  id: 'x',
  list_id: 'list',
  title: 't',
  description: null,
  status: 'open',
  priority: 'medium',
  scheduled_date: null,
  deadline_type: null,
  deadline_date: null,
  deadline_time: null,
  recurrence: null,
  parent_id: null,
  color_label: null,
  reminders: [],
  sound: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  completed_at: null,
  etag: null,
};

const child = (id: string, parent: string, status: TaskStatus): Task => ({
  ...baseTask,
  id,
  parent_id: parent,
  status,
});

describe('deriveStatusFromChildren', () => {
  it('returns null when there are no children', () => {
    expect(deriveStatusFromChildren([])).toBeNull();
  });

  it('any in_progress child wins', () => {
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'completed'),
        child('b', 'p', 'in_progress'),
      ]),
    ).toBe('in_progress');
  });

  it('mixed completed + open with no in_progress → in_progress', () => {
    // Some progress made but not finished.
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'completed'),
        child('b', 'p', 'open'),
      ]),
    ).toBe('in_progress');
  });

  it('all completed → completed', () => {
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'completed'),
        child('b', 'p', 'completed'),
      ]),
    ).toBe('completed');
  });

  it('all cancelled → cancelled', () => {
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'cancelled'),
        child('b', 'p', 'cancelled'),
      ]),
    ).toBe('cancelled');
  });

  it('completed + cancelled (no open, no in_progress) → completed', () => {
    // The non-cancelled work is done; cancelled siblings don't keep
    // the parent unfinished.
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'completed'),
        child('b', 'p', 'cancelled'),
      ]),
    ).toBe('completed');
  });

  it('open + cancelled (no progress) → open', () => {
    // Some intent was dropped, but no work has actually started.
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'open'),
        child('b', 'p', 'cancelled'),
      ]),
    ).toBe('open');
  });

  it('all open → open', () => {
    expect(
      deriveStatusFromChildren([
        child('a', 'p', 'open'),
        child('b', 'p', 'open'),
      ]),
    ).toBe('open');
  });
});

describe('planStatusCascade', () => {
  it('changes only the root when it has no children and no parent', () => {
    const tasks: Task[] = [{ ...baseTask, id: 'root', status: 'open' }];
    expect(planStatusCascade('root', 'completed', tasks)).toEqual([
      { taskId: 'root', status: 'completed' },
    ]);
  });

  it('is a no-op when the new status matches the old', () => {
    const tasks: Task[] = [{ ...baseTask, id: 'root', status: 'completed' }];
    expect(planStatusCascade('root', 'completed', tasks)).toEqual([]);
  });

  it('completing a parent cascades to non-cancelled descendants', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'open' },
      child('a', 'p', 'open'),
      child('b', 'p', 'cancelled'),
      child('c', 'p', 'in_progress'),
    ];
    const writes = planStatusCascade('p', 'completed', tasks);
    const ids = writes.map((w) => `${w.taskId}:${w.status}`).sort();
    // p, a, c flip to completed; b stays cancelled.
    expect(ids).toEqual([
      'a:completed',
      'c:completed',
      'p:completed',
    ]);
  });

  it('cancelling a parent leaves already-completed descendants alone', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'open' },
      child('a', 'p', 'completed'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('p', 'cancelled', tasks);
    const ids = writes.map((w) => `${w.taskId}:${w.status}`).sort();
    expect(ids).toEqual(['b:cancelled', 'p:cancelled']);
  });

  it('marking the last open subtask completed completes the parent', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'in_progress' },
      child('a', 'p', 'completed'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('b', 'completed', tasks);
    // b → completed (root), then parent recomputes to completed.
    expect(writes).toEqual([
      { taskId: 'b', status: 'completed' },
      { taskId: 'p', status: 'completed' },
    ]);
  });

  it('reopening a subtask of a completed parent reopens the parent', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'completed' },
      child('a', 'p', 'completed'),
      child('b', 'p', 'completed'),
    ];
    // Reopen b → mixed completed + open → in_progress on parent.
    const writes = planStatusCascade('b', 'open', tasks);
    expect(writes).toEqual([
      { taskId: 'b', status: 'open' },
      { taskId: 'p', status: 'in_progress' },
    ]);
  });

  it('marking one subtask in_progress lifts the parent to in_progress', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'open' },
      child('a', 'p', 'open'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('a', 'in_progress', tasks);
    expect(writes).toEqual([
      { taskId: 'a', status: 'in_progress' },
      { taskId: 'p', status: 'in_progress' },
    ]);
  });

  it('cancelling one subtask does not cancel the parent', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'open' },
      child('a', 'p', 'open'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('a', 'cancelled', tasks);
    // a → cancelled; b is still open with no completed siblings →
    // parent stays open. No write for p.
    expect(writes).toEqual([{ taskId: 'a', status: 'cancelled' }]);
  });

  it('cancelling the last open subtask cancels the parent', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'open' },
      child('a', 'p', 'cancelled'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('b', 'cancelled', tasks);
    expect(writes).toEqual([
      { taskId: 'b', status: 'cancelled' },
      { taskId: 'p', status: 'cancelled' },
    ]);
  });

  it('propagates through multiple ancestor levels', () => {
    // grandparent → parent → child. Mark the child completed → both
    // ancestors should recompute to completed (each has one child,
    // all completed).
    const tasks: Task[] = [
      { ...baseTask, id: 'gp', status: 'open' },
      { ...baseTask, id: 'p', status: 'open', parent_id: 'gp' },
      { ...baseTask, id: 'c', status: 'open', parent_id: 'p' },
    ];
    const writes = planStatusCascade('c', 'completed', tasks);
    expect(writes).toEqual([
      { taskId: 'c', status: 'completed' },
      { taskId: 'p', status: 'completed' },
      { taskId: 'gp', status: 'completed' },
    ]);
  });

  it('completing a parent recursively completes grandchildren', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'open' },
      { ...baseTask, id: 'c', status: 'open', parent_id: 'p' },
      { ...baseTask, id: 'gc', status: 'open', parent_id: 'c' },
    ];
    const writes = planStatusCascade('p', 'completed', tasks);
    // DFS order: root, then children, then grand-children.
    // Either way, all three should be in the result.
    expect(writes.map((w) => w.taskId).sort()).toEqual(['c', 'gc', 'p']);
    expect(writes.every((w) => w.status === 'completed')).toBe(true);
  });
});

describe('planAncestorRecompute', () => {
  it('reopens a completed parent when a new open child appears', () => {
    // Simulate: child `a` was just created. The store snapshot
    // already contains it as open. Parent was completed because
    // (until this moment) there was only `b` which was completed.
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'completed' },
      child('a', 'p', 'open'),
      child('b', 'p', 'completed'),
    ];
    expect(planAncestorRecompute('p', tasks)).toEqual([
      { taskId: 'p', status: 'in_progress' },
    ]);
  });

  it('completes a parent when the last open child is removed', () => {
    // Snapshot reflects post-deletion: only the completed child
    // remains.
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'in_progress' },
      child('a', 'p', 'completed'),
    ];
    expect(planAncestorRecompute('p', tasks)).toEqual([
      { taskId: 'p', status: 'completed' },
    ]);
  });

  it('is a no-op when the parent already matches its derived status', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p', status: 'in_progress' },
      child('a', 'p', 'in_progress'),
      child('b', 'p', 'open'),
    ];
    expect(planAncestorRecompute('p', tasks)).toEqual([]);
  });
});
