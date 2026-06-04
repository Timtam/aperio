import { describe, expect, it } from 'vitest';

import type { Section, Task } from '../../api/types';
import { buildEntries, DONE_GROUP_ID } from './taskGrouping';

const t = (key: string) => key;

const baseTask = (over: Partial<Task>): Task => ({
  id: 'x',
  list_id: 'L1',
  title: 'x',
  description: null,
  status: 'open',
  priority: 'medium',
  // A date keeps the task out of the cross-list "backlog" bucket so it
  // lands in its per-list group, where section grouping applies.
  scheduled_date: '2026-05-22',
  scheduled_time: null,
  deadline_date: null,
  deadline_time: null,
  recurrence: null,
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
  ...over,
});

const section = (id: string, name: string, order: number): Section => ({
  id,
  list_id: 'L1',
  name,
  order,
});

const listById = new Map([['L1', { name: 'Inbox' }]]);

/** Labels of the separator (group-header) entries, in order. */
function separators(
  result: ReturnType<typeof buildEntries>,
): { label: string; level: number }[] {
  return result.entries
    .filter((e): e is Extract<typeof e, { kind: 'separator' }> =>
      e.kind === 'separator',
    )
    .map((e) => ({ label: e.label, level: e.level ?? 0 }));
}

describe('buildEntries section grouping', () => {
  it('groups tasks by section in declared order, ungrouped first', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's2' }),
      baseTask({ id: 'b', section_id: 's1' }),
      baseTask({ id: 'c', section_id: null }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1), section('s2', 'Doing', 2)],
    });

    // List head (level 0), then the section sub-headers in order. No
    // sub-header precedes the ungrouped task — it just follows the list
    // head.
    expect(separators(result)).toEqual([
      { label: 'Inbox', level: 0 },
      { label: 'To Do', level: 1 },
      { label: 'Doing', level: 1 },
    ]);

    // Task order: ungrouped (c), then s1 (b), then s2 (a).
    const taskIds = result.entries
      .filter((e): e is Extract<typeof e, { kind: 'task' }> => e.kind === 'task')
      .map((e) => e.task.id);
    expect(taskIds).toEqual(['c', 'b', 'a']);
  });

  it('omits headers for empty sections', () => {
    const tasks = [baseTask({ id: 'a', section_id: 's1' })];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1), section('s2', 'Empty', 2)],
    });
    expect(separators(result).map((s) => s.label)).toEqual(['Inbox', 'To Do']);
  });

  it('renders flat (no section headers) when the list has no sections', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's1' }),
      baseTask({ id: 'b' }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {});
    // Only the list head — section_id is ignored when no sections exist.
    expect(separators(result)).toEqual([{ label: 'Inbox', level: 0 }]);
  });

  it('collects completed top-level tasks under a synthetic Done parent', () => {
    const tasks = [
      baseTask({ id: 'open', status: 'open' }),
      baseTask({ id: 'done1', status: 'completed', completed_at: '2026-05-20T10:00:00Z' }),
      baseTask({ id: 'done2', status: 'completed', completed_at: '2026-05-21T10:00:00Z' }),
    ];
    // Expanded: DONE_GROUP_ID is NOT in the collapsed set.
    const expanded = buildEntries(tasks, listById, t, new Set(), {});
    const taskEntries = expanded.entries.filter(
      (e): e is Extract<typeof e, { kind: 'task' }> => e.kind === 'task',
    );
    // Open task (depth 0), then the synthetic group parent (depth 0),
    // then its two completed children (depth 1), most-recent first.
    expect(taskEntries.map((e) => [e.task.id, e.depth, e.hidden])).toEqual([
      ['open', 0, false],
      [DONE_GROUP_ID, 0, false],
      ['done2', 1, false],
      ['done1', 1, false],
    ]);
    const group = taskEntries.find((e) => e.task.id === DONE_GROUP_ID)!;
    expect(group.hasChildren).toBe(true);

    // Collapsed: DONE_GROUP_ID in the set → the children are hidden but
    // the parent header stays visible (stable index space + keyboard nav).
    const collapsed = buildEntries(
      tasks,
      listById,
      t,
      new Set([DONE_GROUP_ID]),
      {},
    );
    const collapsedTasks = collapsed.entries.filter(
      (e): e is Extract<typeof e, { kind: 'task' }> => e.kind === 'task',
    );
    expect(
      collapsedTasks.find((e) => e.task.id === DONE_GROUP_ID)?.hidden,
    ).toBe(false);
    expect(
      collapsedTasks
        .filter((e) => e.task.id.startsWith('done'))
        .every((e) => e.hidden),
    ).toBe(true);
  });

  it('keeps a completed subtask inline under its open parent', () => {
    const tasks = [
      baseTask({ id: 'parent', status: 'open' }),
      baseTask({ id: 'child', parent_id: 'parent', status: 'completed' }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {});
    const ids = result.entries
      .filter((e): e is Extract<typeof e, { kind: 'task' }> => e.kind === 'task')
      .map((e) => e.task.id);
    // No Done group — the only completed task is a subtask, which stays
    // under its parent rather than being hoisted.
    expect(ids).toEqual(['parent', 'child']);
    expect(ids).not.toContain(DONE_GROUP_ID);
  });

  it('omits a list header when the list has only completed tasks', () => {
    const tasks = [baseTask({ id: 'done', status: 'completed' })];
    const result = buildEntries(tasks, listById, t, new Set(), {});
    // No "Inbox" list separator — its only task is in the Done group,
    // which is itself a parent task-row, not a separator heading.
    expect(separators(result).map((s) => s.label)).toEqual([]);
    const ids = result.entries
      .filter((e): e is Extract<typeof e, { kind: 'task' }> => e.kind === 'task')
      .map((e) => e.task.id);
    expect(ids).toEqual([DONE_GROUP_ID, 'done']);
  });

  it('keeps a subtask under its parent regardless of section', () => {
    const tasks = [
      baseTask({ id: 'parent', section_id: 's1' }),
      baseTask({ id: 'child', parent_id: 'parent', section_id: null }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    const taskEntries = result.entries.filter(
      (e): e is Extract<typeof e, { kind: 'task' }> => e.kind === 'task',
    );
    // Child follows its parent depth-first and is indented (depth 1),
    // not hoisted into the ungrouped bucket.
    expect(taskEntries.map((e) => e.task.id)).toEqual(['parent', 'child']);
    expect(taskEntries.find((e) => e.task.id === 'child')!.depth).toBe(1);
  });
});
