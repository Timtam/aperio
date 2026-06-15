import { describe, expect, it } from 'vitest';

import type { Section, Task } from '../../api/types';
import { buildEntries, DONE_GROUP_ID, type Entry } from './taskGrouping';

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
  ...over,
});

const section = (id: string, name: string, order: number): Section => ({
  id,
  list_id: 'L1',
  name,
  color_label: null,
  order,
});

const listById = new Map([['L1', { name: 'Inbox' }]]);

/**
 * Group-header rows (Backlog / a list / a section / the Done group), in
 * DFS order. Headers are now real tree rows — `kind: 'task'` entries that
 * carry {@link Entry.group} meta — so a screen-reader user can arrow onto
 * them; `depth` doubles as the old separator "level".
 */
function headers(
  result: ReturnType<typeof buildEntries>,
): { label: string; level: number; kind: string }[] {
  return result.entries
    .filter((e) => e.group)
    .map((e) => ({
      label: e.task.title,
      level: e.depth,
      kind: e.group!.kind,
    }));
}

/** Real-task rows (everything that is not a group header), in DFS order. */
function taskRows(result: ReturnType<typeof buildEntries>): Entry[] {
  return result.entries.filter((e) => !e.group);
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

    // List head (depth 0), then the section sub-headers in order. No
    // sub-header precedes the ungrouped task — it just follows the list
    // head, one level in.
    expect(headers(result)).toEqual([
      { label: 'Inbox', level: 0, kind: 'list' },
      { label: 'To Do', level: 1, kind: 'section' },
      { label: 'Doing', level: 1, kind: 'section' },
    ]);

    // Task order: ungrouped (c), then s1 (b), then s2 (a).
    expect(taskRows(result).map((e) => e.task.id)).toEqual(['c', 'b', 'a']);
  });

  it('omits headers for empty sections', () => {
    const tasks = [baseTask({ id: 'a', section_id: 's1' })];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1), section('s2', 'Empty', 2)],
    });
    expect(headers(result).map((h) => h.label)).toEqual(['Inbox', 'To Do']);
  });

  it('renders flat (just the list head) when the list has no sections', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's1' }),
      baseTask({ id: 'b' }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {});
    // Only the list head — section_id is ignored when no sections exist.
    expect(headers(result)).toEqual([
      { label: 'Inbox', level: 0, kind: 'list' },
    ]);
    expect(taskRows(result).map((e) => e.task.id)).toEqual(['a', 'b']);
  });

  it('collects completed top-level tasks under a synthetic Done parent', () => {
    const tasks = [
      baseTask({ id: 'open', status: 'open' }),
      baseTask({
        id: 'done1',
        status: 'completed',
        completed_at: '2026-05-20T10:00:00Z',
      }),
      baseTask({
        id: 'done2',
        status: 'completed',
        completed_at: '2026-05-21T10:00:00Z',
      }),
    ];
    // Expanded: DONE_GROUP_ID is NOT in the collapsed set.
    const expanded = buildEntries(tasks, listById, t, new Set(), {});
    const done = expanded.entries.find((e) => e.task.id === DONE_GROUP_ID)!;
    expect(done.group?.kind).toBe('done');
    expect(done.depth).toBe(0);
    expect(done.hasChildren).toBe(true);
    // Completed children sit under the Done header (depth 1), most recent
    // first, and are visible while the group is expanded.
    expect(
      expanded.entries
        .filter((e) => e.task.id.startsWith('done'))
        .map((e) => [e.task.id, e.depth, e.hidden]),
    ).toEqual([
      ['done2', 1, false],
      ['done1', 1, false],
    ]);

    // Collapsed: DONE_GROUP_ID in the set → the children are hidden but
    // the parent header stays visible (stable index space + keyboard nav).
    const collapsed = buildEntries(
      tasks,
      listById,
      t,
      new Set([DONE_GROUP_ID]),
      {},
    );
    expect(
      collapsed.entries.find((e) => e.task.id === DONE_GROUP_ID)?.hidden,
    ).toBe(false);
    expect(
      collapsed.entries
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
    // No Done group — the only completed task is a subtask, which stays
    // under its parent rather than being hoisted.
    expect(result.entries.some((e) => e.task.id === DONE_GROUP_ID)).toBe(false);
    expect(taskRows(result).map((e) => e.task.id)).toEqual(['parent', 'child']);
  });

  it('omits a list header when the list has only completed tasks', () => {
    const tasks = [baseTask({ id: 'done', status: 'completed' })];
    const result = buildEntries(tasks, listById, t, new Set(), {});
    // No list / section header — its only task is in the Done group. The
    // Done header itself is the sole heading.
    expect(headers(result)).toEqual([
      { label: 'views.tasks.done', level: 0, kind: 'done' },
    ]);
    expect(result.entries.map((e) => e.task.id)).toEqual([
      DONE_GROUP_ID,
      'done',
    ]);
  });

  it('keeps a subtask under its parent regardless of section', () => {
    const tasks = [
      baseTask({ id: 'parent', section_id: 's1' }),
      baseTask({ id: 'child', parent_id: 'parent', section_id: null }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    const rows = taskRows(result);
    // Child follows its parent depth-first and is indented one level
    // deeper, not hoisted into the ungrouped bucket.
    expect(rows.map((e) => e.task.id)).toEqual(['parent', 'child']);
    const parent = rows.find((e) => e.task.id === 'parent')!;
    const child = rows.find((e) => e.task.id === 'child')!;
    expect(child.depth).toBe(parent.depth + 1);
  });

  it('groups the backlog by list (1) then section (2)', () => {
    // Unscheduled tasks land in the backlog; it sub-groups by list and
    // section so e.g. Vikunja buckets show even without a scheduled day.
    const tasks = [
      baseTask({ id: 'a', scheduled_date: null, section_id: 's1' }),
      baseTask({ id: 'b', scheduled_date: null, section_id: null }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    expect(headers(result)).toEqual([
      { label: 'views.tasks.backlog', level: 0, kind: 'backlog' },
      { label: 'Inbox', level: 1, kind: 'list' },
      { label: 'To Do', level: 2, kind: 'section' },
    ]);
    // Ungrouped (b) leads, then the section task (a).
    expect(taskRows(result).map((e) => e.task.id)).toEqual(['b', 'a']);
  });

  it('shows a backlog list sub-header even when the list has no sections', () => {
    const tasks = [baseTask({ id: 'a', scheduled_date: null })];
    const result = buildEntries(tasks, listById, t, new Set(), {});
    expect(headers(result)).toEqual([
      { label: 'views.tasks.backlog', level: 0, kind: 'backlog' },
      { label: 'Inbox', level: 1, kind: 'list' },
    ]);
  });

  it('emits a depth-1 structural parent before every nested row', () => {
    // The ArrowLeft "jump to parent" navigation relies on this DFS
    // invariant: every row below the root is preceded by a row exactly one
    // level shallower — its tree parent, be it a group header or a parent
    // task. Resolving the parent by depth (not parent_id) is what lets a
    // top-level task under a header climb to that header.
    const tasks = [
      baseTask({ id: 'p', scheduled_date: null, section_id: 's1' }),
      baseTask({
        id: 'c',
        parent_id: 'p',
        scheduled_date: null,
        section_id: null,
      }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    // Backlog(0) → Inbox(1) → To Do(2) → p(3) → c(4): a strictly nested chain.
    expect(result.entries.map((e) => e.depth)).toEqual([0, 1, 2, 3, 4]);
    result.entries.forEach((entry, idx) => {
      if (entry.depth === 0) return;
      let parent: (typeof result.entries)[number] | null = null;
      for (let i = idx - 1; i >= 0; i--) {
        if (result.entries[i].depth === entry.depth - 1) {
          parent = result.entries[i];
          break;
        }
      }
      expect(parent, `${entry.task.id} (depth ${entry.depth})`).not.toBeNull();
    });
  });

  it('makes every row navigable (flatTasks aligns with entries)', () => {
    // Group headers are real tree rows: each entry has a matching
    // flatTasks slot at its own index, so arrow-key nav reaches headers
    // and tasks alike.
    const tasks = [
      baseTask({ id: 'a', scheduled_date: null, section_id: 's1' }),
    ];
    const result = buildEntries(tasks, listById, t, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    expect(result.flatTasks).toHaveLength(result.entries.length);
    result.entries.forEach((e) => {
      expect(result.flatTasks[e.index].id).toBe(e.task.id);
    });
    // Backlog → Inbox → To Do → task a : four navigable rows.
    expect(result.flatTasks).toHaveLength(4);
  });
});
