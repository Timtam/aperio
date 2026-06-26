import { describe, expect, it } from 'vitest';

import type { Section, Task, TaskUser } from '../../api/types';
import {
  buildEntries,
  DEFERRED_GROUP_ID,
  DONE_GROUP_ID,
  isTaskDeferred,
  type Entry,
} from './taskGrouping';

const t = (key: string) => key;

/** Fixed "today" for the deferred (resurface) gate. */
const TODAY = '2026-05-21';

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

/** `buildEntries` with the fixtures' list map, identity `t`, and a fixed
 *  "today" pre-applied. */
const build = (
  tasks: Task[],
  collapsed: Set<string> = new Set(),
  sectionsByList: Record<string, Section[]> = {},
) => buildEntries(tasks, listById, t, collapsed, sectionsByList, TODAY);

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

describe('buildEntries done-count split', () => {
  const me: TaskUser = { id: 'u1', name: 'Me', email: null };
  const other: TaskUser = { id: 'u2', name: 'Other', email: null };
  const done = (id: string, assignees: TaskUser[]): Task =>
    baseTask({ id, status: 'completed', completed_at: '2026-05-20T10:00:00Z', assignees });
  const doneTitle = (result: ReturnType<typeof buildEntries>): string | undefined =>
    headers(result).find((h) => h.kind === 'done')?.label;

  it('splits the Done title when a done task is owned by someone else', () => {
    const tasks = [done('a', []), done('b', [me]), done('c', [other])];
    const result = buildEntries(tasks, listById, t, new Set(), {}, TODAY, { L1: me });
    expect(doneTitle(result)).toBe('views.tasks.doneSplit');
  });

  it('keeps one Done count when everything is mine or unassigned', () => {
    const tasks = [done('a', []), done('b', [me])];
    const result = buildEntries(tasks, listById, t, new Set(), {}, TODAY, { L1: me });
    expect(doneTitle(result)).toBe('views.tasks.done');
  });

  it('keeps one Done count without an identity for the list', () => {
    const tasks = [done('a', [other])];
    // No currentUserByList ⇒ {} ⇒ everything counts as mine ⇒ single count.
    expect(doneTitle(build(tasks))).toBe('views.tasks.done');
  });
});

describe('buildEntries section grouping', () => {
  it('groups tasks by section in declared order, ungrouped first', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's2' }),
      baseTask({ id: 'b', section_id: 's1' }),
      baseTask({ id: 'c', section_id: null }),
    ];
    const result = build(tasks, new Set(), {
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
    const result = build(tasks, new Set(), {
      L1: [section('s1', 'To Do', 1), section('s2', 'Empty', 2)],
    });
    expect(headers(result).map((h) => h.label)).toEqual(['Inbox', 'To Do']);
  });

  it('renders flat (just the list head) when the list has no sections', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's1' }),
      baseTask({ id: 'b' }),
    ];
    const result = build(tasks);
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
    const expanded = build(tasks);
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
    const collapsed = build(tasks, new Set([DONE_GROUP_ID]));
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
    const result = build(tasks);
    // No Done group — the only completed task is a subtask, which stays
    // under its parent rather than being hoisted.
    expect(result.entries.some((e) => e.task.id === DONE_GROUP_ID)).toBe(false);
    expect(taskRows(result).map((e) => e.task.id)).toEqual(['parent', 'child']);
  });

  it('omits a list header when the list has only completed tasks', () => {
    const tasks = [baseTask({ id: 'done', status: 'completed' })];
    const result = build(tasks);
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
    const result = build(tasks, new Set(), {
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
    const result = build(tasks, new Set(), {
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
    const result = build(tasks);
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
    const result = build(tasks, new Set(), {
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
    const result = build(tasks, new Set(), {
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

describe('buildEntries natural ordering', () => {
  it('sorts tasks within a section natural-ascending by title', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's1', title: 'Aufgabe 10' }),
      baseTask({ id: 'b', section_id: 's1', title: 'Aufgabe 2' }),
      baseTask({ id: 'c', section_id: 's1', title: 'Aufgabe 1' }),
    ];
    const result = build(tasks, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    // Numeric-aware: "2" sorts before "10", not lexicographically after it.
    expect(taskRows(result).map((e) => e.task.title)).toEqual([
      'Aufgabe 1',
      'Aufgabe 2',
      'Aufgabe 10',
    ]);
  });

  it('floats higher priority above natural order within a section', () => {
    const tasks = [
      baseTask({ id: 'a', section_id: 's1', title: 'Apple', priority: 'medium' }),
      baseTask({ id: 'b', section_id: 's1', title: 'Zebra', priority: 'high' }),
    ];
    const result = build(tasks, new Set(), {
      L1: [section('s1', 'To Do', 1)],
    });
    // High priority wins the band; natural order only orders within a band.
    expect(taskRows(result).map((e) => e.task.title)).toEqual(['Zebra', 'Apple']);
  });

  it('sorts ungrouped backlog tasks naturally too', () => {
    const tasks = [
      baseTask({ id: 'a', scheduled_date: null, title: 'Item 10' }),
      baseTask({ id: 'b', scheduled_date: null, title: 'Item 2' }),
    ];
    expect(taskRows(build(tasks)).map((e) => e.task.title)).toEqual([
      'Item 2',
      'Item 10',
    ]);
  });

  it('sorts subtask siblings naturally', () => {
    const tasks = [
      baseTask({ id: 'p', title: 'Parent' }),
      baseTask({ id: 'c10', parent_id: 'p', title: 'Step 10' }),
      baseTask({ id: 'c2', parent_id: 'p', title: 'Step 2' }),
    ];
    expect(taskRows(build(tasks)).map((e) => e.task.title)).toEqual([
      'Parent',
      'Step 2',
      'Step 10',
    ]);
  });
});

describe('isTaskDeferred', () => {
  it('is true only for a future resurface date', () => {
    expect(
      isTaskDeferred(baseTask({ resurface_date: '2026-10-01' }), TODAY),
    ).toBe(true);
    // today or past ⇒ active now, not deferred
    expect(isTaskDeferred(baseTask({ resurface_date: TODAY }), TODAY)).toBe(
      false,
    );
    expect(
      isTaskDeferred(baseTask({ resurface_date: '2026-01-01' }), TODAY),
    ).toBe(false);
    // no resurface date ⇒ never deferred
    expect(isTaskDeferred(baseTask({ resurface_date: null }), TODAY)).toBe(
      false,
    );
  });
});

describe('buildEntries deferred (Zukünftig) grouping', () => {
  it('holds a future-resurface backlog task in the Deferred group', () => {
    const tasks = [
      baseTask({ id: 'now', scheduled_date: null }),
      baseTask({
        id: 'later',
        scheduled_date: null,
        resurface_date: '2026-10-01',
      }),
    ];
    const result = build(tasks);
    // The future task is NOT in the active backlog…
    const backlogChildIds = taskRows(result).map((e) => e.task.id);
    expect(backlogChildIds).toContain('now');
    // …it lives under the Deferred header instead.
    const deferred = result.entries.find(
      (e) => e.task.id === DEFERRED_GROUP_ID,
    )!;
    expect(deferred.group?.kind).toBe('deferred');
    expect(deferred.depth).toBe(0);
    const deferredChildren = result.entries.filter(
      (e) => !e.group && e.depth === deferred.depth + 1 && e.index > deferred.index,
    );
    expect(deferredChildren.map((e) => e.task.id)).toEqual(['later']);
  });

  it('treats a resurface date of today (or past) as visible now', () => {
    const tasks = [
      baseTask({ id: 'today', scheduled_date: null, resurface_date: TODAY }),
      baseTask({ id: 'past', scheduled_date: null, resurface_date: '2026-01-01' }),
    ];
    const result = build(tasks);
    // Neither is deferred — a resurface date that's already arrived means
    // the task belongs in the active backlog.
    expect(result.entries.some((e) => e.task.id === DEFERRED_GROUP_ID)).toBe(
      false,
    );
    expect(taskRows(result).map((e) => e.task.id).sort()).toEqual([
      'past',
      'today',
    ]);
  });

  it('orders deferred tasks soonest-resurface first', () => {
    const tasks = [
      baseTask({ id: 'oct', scheduled_date: null, resurface_date: '2026-10-01' }),
      baseTask({ id: 'jun', scheduled_date: null, resurface_date: '2026-06-01' }),
    ];
    const result = build(tasks);
    const deferred = result.entries.find(
      (e) => e.task.id === DEFERRED_GROUP_ID,
    )!;
    const children = result.entries.filter(
      (e) => !e.group && e.index > deferred.index,
    );
    expect(children.map((e) => e.task.id)).toEqual(['jun', 'oct']);
  });

  it('starts the Deferred group collapsed via the collapsed set', () => {
    const tasks = [
      baseTask({
        id: 'later',
        scheduled_date: null,
        resurface_date: '2026-10-01',
      }),
    ];
    const result = build(tasks, new Set([DEFERRED_GROUP_ID]));
    expect(
      result.entries.find((e) => e.task.id === DEFERRED_GROUP_ID)?.hidden,
    ).toBe(false);
    expect(
      result.entries
        .filter((e) => e.task.id === 'later')
        .every((e) => e.hidden),
    ).toBe(true);
  });
});
