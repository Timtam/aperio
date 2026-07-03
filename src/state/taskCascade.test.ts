import { describe, expect, it } from 'vitest';

import type { Task, TaskStatus } from '../api/types';
import {
  autoDateOnStart,
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
  effort: 'medium',
  scheduled_date: null,
  scheduled_time: null,
  deadline_date: null,
  deadline_time: null,
  deadline_reminder_days: null,
  recurrence: null,
  resurface_date: null,
  series_id: null,
  parent_id: null,
  section_id: null,
  color_label: null,
  reminders: [],
  assignees: [],
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

describe('autoDateOnStart', () => {
  const TODAY = '2026-06-06';

  it('pins a dateless task moving into in_progress to today', () => {
    expect(autoDateOnStart('in_progress', null, TODAY)).toBe(TODAY);
  });

  it('leaves a task that already has a scheduled date alone', () => {
    expect(autoDateOnStart('in_progress', '2026-06-01', TODAY)).toBeUndefined();
  });

  it('only fires for in_progress', () => {
    expect(autoDateOnStart('open', null, TODAY)).toBeUndefined();
    expect(autoDateOnStart('completed', null, TODAY)).toBeUndefined();
    expect(autoDateOnStart('cancelled', null, TODAY)).toBeUndefined();
  });

  it('is a no-op when the auto-date setting is off (no todayKey)', () => {
    expect(autoDateOnStart('in_progress', null, undefined)).toBeUndefined();
  });
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

describe('parent cycles (external-provider data)', () => {
  // External providers can hand us a parent LOOP (e.g. two Vikunja tasks
  // each relating to the other). No planner may spin forever on it —
  // before the visited guards, a single check-off froze the whole app.
  const cycle = (aStatus: TaskStatus, bStatus: TaskStatus): Task[] => [
    child('a', 'b', aStatus),
    child('b', 'a', bStatus),
  ];

  it('terminates the down-cascade on a two-task cycle', () => {
    const writes = planStatusCascade('a', 'completed', cycle('open', 'open'));
    // a is the root write; b follows as its "descendant"; then the walk
    // stops instead of alternating a→b→a forever.
    expect(writes).toEqual([
      { taskId: 'a', status: 'completed' },
      { taskId: 'b', status: 'completed' },
    ]);
  });

  it('terminates the down-cascade when the cycle is already at the target', () => {
    const writes = planStatusCascade('a', 'completed', [
      child('a', 'b', 'open'),
      child('b', 'a', 'completed'),
    ]);
    expect(writes).toEqual([{ taskId: 'a', status: 'completed' }]);
  });

  it('terminates the up-cascade on a cycle', () => {
    // Reopening skips the down-cascade and climbs parents — the climb
    // must stop when it re-reaches a visited node.
    const writes = planStatusCascade('a', 'open', cycle('completed', 'completed'));
    expect(writes.length).toBeGreaterThan(0);
    expect(writes.length).toBeLessThan(10);
  });

  it('terminates planAncestorRecompute on a cycle', () => {
    const writes = planAncestorRecompute('a', cycle('open', 'completed'));
    // Each member is recomputed at most once; no infinite climb.
    expect(writes.length).toBeLessThan(10);
  });
});

describe('auto-date (todayKey)', () => {
  const TODAY = '2026-05-21';

  it('pins a dateless task transitioning to in_progress', () => {
    const tasks = [
      { ...baseTask, id: 'a', status: 'open' as const, scheduled_date: null },
    ];
    const writes = planStatusCascade('a', 'in_progress', tasks, {
      todayKey: TODAY,
    });
    expect(writes).toEqual([
      { taskId: 'a', status: 'in_progress', scheduledDate: TODAY },
    ]);
  });

  it('leaves an already-dated task alone when it goes in_progress', () => {
    const tasks = [
      {
        ...baseTask,
        id: 'a',
        status: 'open' as const,
        scheduled_date: '2026-05-19',
      },
    ];
    const writes = planStatusCascade('a', 'in_progress', tasks, {
      todayKey: TODAY,
    });
    expect(writes).toEqual([{ taskId: 'a', status: 'in_progress' }]);
  });

  it('does not fire for transitions to non-in_progress statuses', () => {
    const tasks = [
      { ...baseTask, id: 'a', status: 'open' as const, scheduled_date: null },
    ];
    expect(
      planStatusCascade('a', 'completed', tasks, { todayKey: TODAY }),
    ).toEqual([{ taskId: 'a', status: 'completed' }]);
    expect(
      planStatusCascade('a', 'cancelled', tasks, { todayKey: TODAY }),
    ).toEqual([{ taskId: 'a', status: 'cancelled' }]);
  });

  it('fires for reactivation (completed → in_progress) on a dateless task', () => {
    const tasks = [
      {
        ...baseTask,
        id: 'a',
        status: 'completed' as const,
        scheduled_date: null,
      },
    ];
    const writes = planStatusCascade('a', 'in_progress', tasks, {
      todayKey: TODAY,
    });
    expect(writes).toEqual([
      { taskId: 'a', status: 'in_progress', scheduledDate: TODAY },
    ]);
  });

  it('also pins a dateless parent that the up-cascade derives to in_progress', () => {
    const tasks = [
      {
        ...baseTask,
        id: 'p',
        status: 'open' as const,
        scheduled_date: null,
      },
      child('a', 'p', 'open'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('a', 'in_progress', tasks, {
      todayKey: TODAY,
    });
    // a → in_progress (also pinned), and p re-derives to in_progress
    // (also pinned because it was dateless).
    expect(writes).toEqual([
      { taskId: 'a', status: 'in_progress', scheduledDate: TODAY },
      { taskId: 'p', status: 'in_progress', scheduledDate: TODAY },
    ]);
  });

  it('still fires when cascade coupling is off (orthogonal feature)', () => {
    const tasks = [
      { ...baseTask, id: 'a', status: 'open' as const, scheduled_date: null },
    ];
    const writes = planStatusCascade('a', 'in_progress', tasks, {
      cascadeEnabled: false,
      todayKey: TODAY,
    });
    expect(writes).toEqual([
      { taskId: 'a', status: 'in_progress', scheduledDate: TODAY },
    ]);
  });

  it('planAncestorRecompute pins a dateless parent it newly derives to in_progress', () => {
    const tasks = [
      {
        ...baseTask,
        id: 'p',
        status: 'completed' as const,
        scheduled_date: null,
      },
      // A freshly-added open subtask drags the parent off "completed"
      // — the rule "completed + open" reads as in_progress.
      child('a', 'p', 'completed'),
      child('b', 'p', 'open'),
    ];
    const writes = planAncestorRecompute('p', tasks, { todayKey: TODAY });
    expect(writes).toEqual([
      { taskId: 'p', status: 'in_progress', scheduledDate: TODAY },
    ]);
  });

  it('does nothing when todayKey is omitted', () => {
    const tasks = [
      { ...baseTask, id: 'a', status: 'open' as const, scheduled_date: null },
    ];
    expect(planStatusCascade('a', 'in_progress', tasks)).toEqual([
      { taskId: 'a', status: 'in_progress' },
    ]);
  });
});

describe('decoupled mode (cascadeEnabled = false)', () => {
  it('planStatusCascade returns only the root write — no down-cascade', () => {
    const tasks = [
      { ...baseTask, id: 'p', status: 'open' as const },
      child('a', 'p', 'open'),
      child('b', 'p', 'open'),
    ];
    const writes = planStatusCascade('p', 'completed', tasks, {
      cascadeEnabled: false,
    });
    expect(writes).toEqual([{ taskId: 'p', status: 'completed' }]);
  });

  it('planStatusCascade returns only the root write — no up-cascade', () => {
    const tasks = [
      { ...baseTask, id: 'p', status: 'open' as const },
      child('a', 'p', 'open'),
      child('b', 'p', 'completed'),
    ];
    const writes = planStatusCascade('a', 'completed', tasks, {
      cascadeEnabled: false,
    });
    expect(writes).toEqual([{ taskId: 'a', status: 'completed' }]);
  });

  it('planStatusCascade returns [] when the status already matches', () => {
    const tasks = [{ ...baseTask, id: 'p', status: 'completed' as const }];
    const writes = planStatusCascade('p', 'completed', tasks, {
      cascadeEnabled: false,
    });
    expect(writes).toEqual([]);
  });

  it('planAncestorRecompute is a no-op', () => {
    const tasks = [
      { ...baseTask, id: 'p', status: 'open' as const },
      child('a', 'p', 'completed'),
      child('b', 'p', 'completed'),
    ];
    expect(
      planAncestorRecompute('p', tasks, { cascadeEnabled: false }),
    ).toEqual([]);
  });
});
