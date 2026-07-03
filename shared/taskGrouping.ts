import type { Section, Task, TaskUser } from './types';
import { classifyDoneByMe } from './taskAssignment';
import { priorityRank } from './taskStatus';

/** Sentinel id of the synthetic "Done (N)" group row. */
export const DONE_GROUP_ID = '__aperio_done_group__';
/** Sentinel id of the synthetic "Backlog" group row. */
export const BACKLOG_GROUP_ID = '__aperio_backlog_group__';
/** Sentinel id of the synthetic "Zukünftig (N)" group row — backlog tasks
 *  whose `resurface_date` is still in the future (DESIGN §9.12). */
export const DEFERRED_GROUP_ID = '__aperio_deferred_group__';

/**
 * How the task view groups its top level:
 *  - `'state'` — by lifecycle: Backlog → per-list scheduled groups → Zukünftig
 *    → Done (the historical grouping).
 *  - `'list'` — every NON-completed task in its own list (+ sections),
 *    regardless of backlog/scheduled/deferred state; only Done stays separate.
 */
export type TaskGroupBy = 'state' | 'list';

/**
 * A backlog task is **deferred** when its `resurface_date` is strictly after
 * `today` (`YYYY-MM-DD`): it's waiting to come back and must be held out of
 * the active backlog (DESIGN §9.3 / §9.12). The single source of truth for
 * every backlog surface — the task view's grouping AND the week/month backlog
 * rail — so they can't drift apart and show the same task in two places.
 */
export function isTaskDeferred(task: Task, today: string): boolean {
  return task.resurface_date != null && task.resurface_date > today;
}

/** Metadata carried by a group-header row (Backlog / a list / a section /
 *  the synthetic Done or Deferred group). When `group` is set on an
 *  {@link Entry}, the row is a collapsible header rather than a real task. */
export interface GroupMeta {
  kind: 'backlog' | 'list' | 'section' | 'done' | 'deferred';
  /** Section row id — present only for `kind: 'section'`; lets the header
   *  tint to the section colour and offer the ⋮ actions. */
  sectionId?: string;
  /** The owning list — present for list + section headers. */
  listId?: string;
  /** The section row itself (for the colour + ⋮ menu + drop target). */
  section?: Section;
}

/**
 * One row in the flattened task tree the TaskView renders. Every row is a
 * `treeitem`: real tasks, plus the synthetic group headers (Backlog, each
 * list, each section, the Done group) which carry {@link GroupMeta}. Headers
 * being real tree rows is what lets a screen-reader user arrow onto them and
 * hear "Backlog, level 1, collapsed" instead of the grouping living only in
 * each task's label.
 */
export interface Entry {
  kind: 'task';
  task: Task;
  listName: string;
  /** Position in `flatTasks` — what `focusIndex` indexes into. */
  index: number;
  /** 0 for a top-level row, +1 per nesting level (drives aria-level + indent). */
  depth: number;
  /** Set when this row has at least one child row. */
  hasChildren: boolean;
  /** True when an ancestor row is collapsed (renderer skips it; the index
   *  space stays stable so keyboard nav can clamp). */
  hidden: boolean;
  /** Present when this row is a group header rather than a real task. */
  group?: GroupMeta;
}

/** Minimal synthetic Task standing in for a group header. Only its `id`
 *  (collapse key + activedescendant target) and `parent_id` (Left-arrow
 *  parent jump) matter — the render branches on `entry.group` before it
 *  would read any task-shaped field. */
function groupTask(
  id: string,
  title: string,
  listId: string,
  parentId: string | null,
): Task {
  return {
    id,
    list_id: listId,
    title,
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
    parent_id: parentId,
    section_id: null,
    color_label: null,
    reminders: [],
    assignees: [],
    sound: null,
    created_at: '',
    updated_at: '',
    completed_at: null,
    etag: null,
  };
}

/** A node in the group forest built below: either a collapsible group header
 *  or a real task (whose own subtasks are resolved at emit time). */
type GNode =
  | {
      t: 'group';
      id: string;
      title: string;
      meta: GroupMeta;
      children: GNode[];
    }
  | { t: 'task'; task: Task };

/** Natural (numeric-aware) ascending title compare: "Aufgabe 2" sorts before
 *  "Aufgabe 10", not after. */
function naturalCompare(a: string, b: string): number {
  return a.localeCompare(b, undefined, { numeric: true });
}

/** Sibling order within a group / under a parent: high priority floats up
 *  (unchanged), then natural ascending title — replacing the old insertion-order
 *  tiebreaker so each group reads A→Z within a priority band. Exported as THE
 *  task ordering: the calendar day surfaces (`filterTasksOnDay`) sort a day's
 *  tasks with the same comparator so the planner reads like the task list. */
export function taskOrder(a: Task, b: Task): number {
  return (
    priorityRank(a.priority) - priorityRank(b.priority) ||
    naturalCompare(a.title, b.title)
  );
}

export function buildEntries(
  tasks: Task[],
  taskListById: Map<string, { name: string }>,
  t: (key: string, vars?: Record<string, unknown>) => string,
  collapsed: Set<string>,
  sectionsByList: Record<string, Section[]>,
  /** Today as `YYYY-MM-DD`; tasks whose `resurface_date` is strictly after
   *  it are held back in the "Zukünftig" group (DESIGN §9.12). */
  today: string,
  /** Connected user per list id (Vikunja & co.), used to split the Done count
   *  into "mine vs others". Lists without an identity are absent/null and never
   *  split. */
  currentUserByList: Record<string, TaskUser | null> = {},
  /** Top-level grouping (see {@link TaskGroupBy}). Defaults to `'state'` so
   *  existing callers keep the historical lifecycle grouping. */
  groupBy: TaskGroupBy = 'state',
): { entries: Entry[]; flatTasks: Task[] } {
  // Bucket children under their parent for O(1) subtask lookup. Tasks whose
  // parent_id points at a missing row are orphans → surfaced at top level.
  const childrenByParent = new Map<string, Task[]>();
  const allIds = new Set<string>();
  tasks.forEach((task) => allIds.add(task.id));
  const topLevel: Task[] = [];
  tasks.forEach((task) => {
    if (task.parent_id && allIds.has(task.parent_id)) {
      const bucket = childrenByParent.get(task.parent_id) ?? [];
      bucket.push(task);
      childrenByParent.set(task.parent_id, bucket);
    } else {
      topLevel.push(task);
    }
  });
  // Subtask siblings sort the same way as top-level tasks (priority band, then
  // natural title) — so a parent's children also read A→Z, not add-order.
  childrenByParent.forEach((bucket) => bucket.sort(taskOrder));

  // Cycle guard: parent links can come from external providers (e.g. two
  // Vikunja tasks each carrying a `parenttask` relation onto the other), and
  // a parent CYCLE has no top-level member — without this, every task in the
  // cycle (plus its subtree) would silently vanish from the view, and
  // emitting one would recurse forever. Chase each unreachable task's parent
  // chain onto its cycle and promote that member to top level, cutting its
  // parent edge in the LOCAL buckets only (the task object itself is shared
  // state and stays untouched) so the whole cluster renders as a tree again.
  const reachable = new Set<string>();
  const markReachable = (task: Task) => {
    if (reachable.has(task.id)) return;
    reachable.add(task.id);
    (childrenByParent.get(task.id) ?? []).forEach(markReachable);
  };
  topLevel.forEach(markReachable);
  if (reachable.size < tasks.length) {
    const byId = new Map(tasks.map((task) => [task.id, task]));
    tasks.forEach((task) => {
      if (reachable.has(task.id)) return;
      // Walk up until the next parent would revisit the chain — `member` is
      // then on the cycle. (Every parent exists here: a missing parent would
      // have put the task in `topLevel` and made the cluster reachable.)
      let member = task;
      const walked = new Set<string>([member.id]);
      while (member.parent_id) {
        const parent = byId.get(member.parent_id);
        if (!parent || walked.has(parent.id)) break;
        walked.add(parent.id);
        member = parent;
      }
      const parentId = member.parent_id;
      if (parentId) {
        const bucket = childrenByParent.get(parentId) ?? [];
        childrenByParent.set(
          parentId,
          bucket.filter((child) => child.id !== member.id),
        );
      }
      topLevel.push(member);
      markReachable(member);
    });
  }

  // Total tasks contained under one task (its whole subtask subtree) and under
  // a list of tasks (each task + its subtree) — drives the "(N)" count
  // indicators on group headers and tasks-with-subtasks, matching the existing
  // Done / Zukünftig group counts.
  const countSubtasks = (taskId: string): number =>
    (childrenByParent.get(taskId) ?? []).reduce(
      (n, s) => n + 1 + countSubtasks(s.id),
      0,
    );
  const totalUnder = (items: Task[]): number =>
    items.reduce((n, task) => n + 1 + countSubtasks(task.id), 0);

  // Completed top-level tasks (with their subtree) collapse into a single
  // "Done (N)" group; the active groups show only open work. A completed
  // *subtask* under an open parent stays inline.
  const doneTopLevel: Task[] = [];
  const openTopLevel: Task[] = [];
  topLevel.forEach((task) => {
    if (task.status === 'completed') doneTopLevel.push(task);
    else openTopLevel.push(task);
  });

  // High priority floats up, low sinks; within a band, natural ascending title
  // (`taskOrder`) — so each group reads A→Z instead of add-order. Every
  // downstream bucket is filled by walking `openTopLevel` in order, so this one
  // sort covers backlog + scheduled lists + sections + ungrouped alike.
  openTopLevel.sort(taskOrder);

  // Deferred (DESIGN §9.12): a backlog task whose resurface day is still in
  // the future is held out of the active groups and collected under
  // "Zukünftig" — it's neither lost nor cluttering today's work. The same
  // gate doubles as the §9.3 backlog filter (see `isTaskDeferred`).
  const deferred: Task[] = [];
  const active: Task[] = [];
  openTopLevel.forEach((task) => {
    if (isTaskDeferred(task, today)) deferred.push(task);
    else active.push(task);
  });

  // Backlog (no planned work day) vs the per-list groups (scheduled).
  const backlog: Task[] = [];
  const byList = new Map<string, Task[]>();
  active.forEach((task) => {
    if (!task.scheduled_date) {
      backlog.push(task);
      return;
    }
    const bucket = byList.get(task.list_id) ?? [];
    bucket.push(task);
    byList.set(task.list_id, bucket);
  });

  const nameOf = (listId: string) => taskListById.get(listId)?.name ?? listId;
  const byName = (a: string, b: string) => nameOf(a).localeCompare(nameOf(b));

  // A list's tasks → [ungrouped task nodes] + a section group node per
  // non-empty section (in declared order). `idScope` keeps the synthetic
  // section ids unique between a list's backlog and scheduled appearances.
  const listChildren = (
    listId: string,
    items: Task[],
    idScope: string,
  ): GNode[] => {
    const sections = sectionsByList[listId] ?? [];
    if (sections.length === 0) {
      return items.map((task) => ({ t: 'task', task }) as GNode);
    }
    const sectionIds = new Set(sections.map((s) => s.id));
    const bySection = new Map<string, Task[]>();
    const ungrouped: Task[] = [];
    items.forEach((task) => {
      if (task.section_id && sectionIds.has(task.section_id)) {
        const arr = bySection.get(task.section_id) ?? [];
        arr.push(task);
        bySection.set(task.section_id, arr);
      } else {
        ungrouped.push(task);
      }
    });
    const out: GNode[] = ungrouped.map((task) => ({ t: 'task', task }) as GNode);
    [...sections]
      .sort((a, b) => a.order - b.order)
      .forEach((section) => {
        const secTasks = bySection.get(section.id);
        if (!secTasks || secTasks.length === 0) return;
        out.push({
          t: 'group',
          id: `grp:sec:${idScope}:${section.id}`,
          title: `${section.name} (${totalUnder(secTasks)})`,
          meta: { kind: 'section', sectionId: section.id, listId, section },
          children: secTasks.map((task) => ({ t: 'task', task }) as GNode),
        });
      });
    return out;
  };

  const forest: GNode[] = [];

  // 'list' mode: every non-completed task in its own list (+ sections),
  // regardless of backlog/scheduled/deferred state — only Done stays separate
  // (appended below, same as the state grouping). `openTopLevel` is already the
  // full non-completed set (the deferred/backlog split happened after it), so
  // grouping it by list folds Backlog + scheduled + Zukünftig into one list row.
  if (groupBy === 'list') {
    const byListAll = new Map<string, Task[]>();
    openTopLevel.forEach((task) => {
      const arr = byListAll.get(task.list_id) ?? [];
      arr.push(task);
      byListAll.set(task.list_id, arr);
    });
    Array.from(byListAll.entries())
      .sort(([a], [b]) => byName(a, b))
      .forEach(([listId, items]) => {
        forest.push({
          t: 'group',
          id: `grp:list:${listId}`,
          title: `${nameOf(listId)} (${totalUnder(items)})`,
          meta: { kind: 'list', listId },
          children: listChildren(listId, items, `ls:${listId}`),
        });
      });
  }

  // Backlog → list → section. Grouping the backlog (not just the scheduled
  // tasks) is what makes e.g. a Vikunja project's buckets visible even when
  // nothing is scheduled.
  if (groupBy === 'state' && backlog.length > 0) {
    const backlogByList = new Map<string, Task[]>();
    backlog.forEach((task) => {
      const arr = backlogByList.get(task.list_id) ?? [];
      arr.push(task);
      backlogByList.set(task.list_id, arr);
    });
    const listNodes: GNode[] = Array.from(backlogByList.entries())
      .sort(([a], [b]) => byName(a, b))
      .map(([listId, items]) => ({
        t: 'group',
        id: `grp:bl:list:${listId}`,
        title: `${nameOf(listId)} (${totalUnder(items)})`,
        meta: { kind: 'list', listId },
        children: listChildren(listId, items, `bl:${listId}`),
      }));
    forest.push({
      t: 'group',
      id: BACKLOG_GROUP_ID,
      title: `${t('views.tasks.backlog')} (${totalUnder(backlog)})`,
      meta: { kind: 'backlog' },
      children: listNodes,
    });
  }

  // Scheduled per-list groups, sorted by list name.
  if (groupBy === 'state') {
    Array.from(byList.entries())
      .sort(([a], [b]) => byName(a, b))
      .forEach(([listId, items]) => {
        forest.push({
          t: 'group',
          id: `grp:sc:list:${listId}`,
          title: `${nameOf(listId)} (${totalUnder(items)})`,
          meta: { kind: 'list', listId },
          children: listChildren(listId, items, `sc:${listId}`),
        });
      });
  }

  // "Zukünftig" group: backlog tasks waiting to resurface, soonest first.
  // Sits just before Done — both are end-of-list, navigable, collapsible.
  // (State mode only — in list mode these live in their list group above.)
  if (groupBy === 'state' && deferred.length > 0) {
    deferred.sort((a, b) =>
      (a.resurface_date ?? '').localeCompare(b.resurface_date ?? ''),
    );
    forest.push({
      t: 'group',
      id: DEFERRED_GROUP_ID,
      title: t('views.tasks.deferred', { count: deferred.length }),
      meta: { kind: 'deferred' },
      children: deferred.map((task) => ({ t: 'task', task }) as GNode),
    });
  }

  // Done group last, most-recently-completed first.
  if (doneTopLevel.length > 0) {
    doneTopLevel.sort((a, b) =>
      (b.completed_at ?? '').localeCompare(a.completed_at ?? ''),
    );
    // Split the count into mine (unassigned OR assigned to me) vs others
    // (assigned to a concrete other user) when at least one done task is
    // someone else's; otherwise a single count (personal lists never split).
    const mineCount = doneTopLevel.filter(
      (task) =>
        classifyDoneByMe(task.assignees, currentUserByList[task.list_id] ?? null) ===
        'me',
    ).length;
    const othersCount = doneTopLevel.length - mineCount;
    forest.push({
      t: 'group',
      id: DONE_GROUP_ID,
      title:
        othersCount > 0
          ? t('views.tasks.doneSplit', { mine: mineCount, others: othersCount })
          : t('views.tasks.done', { count: doneTopLevel.length }),
      meta: { kind: 'done' },
      children: doneTopLevel.map((task) => ({ t: 'task', task }) as GNode),
    });
  }

  // Depth-first emit. Hidden rows still join `flatTasks` so the index space
  // stays stable across collapse; the renderer skips them.
  const entries: Entry[] = [];
  const flatTasks: Task[] = [];

  const emitTask = (task: Task, depth: number, hidden: boolean) => {
    const subtasks = childrenByParent.get(task.id) ?? [];
    entries.push({
      kind: 'task',
      task,
      listName: nameOf(task.list_id),
      index: flatTasks.length,
      depth,
      hasChildren: subtasks.length > 0,
      hidden,
    });
    flatTasks.push(task);
    const childHidden = hidden || collapsed.has(task.id);
    subtasks.forEach((child) => emitTask(child, depth + 1, childHidden));
  };

  const emitNode = (node: GNode, depth: number, hidden: boolean, parentId: string | null) => {
    if (node.t === 'task') {
      emitTask(node.task, depth, hidden);
      return;
    }
    const synthetic = groupTask(node.id, node.title, node.meta.listId ?? '', parentId);
    entries.push({
      kind: 'task',
      task: synthetic,
      listName: '',
      index: flatTasks.length,
      depth,
      hasChildren: node.children.length > 0,
      hidden,
      group: node.meta,
    });
    flatTasks.push(synthetic);
    const childHidden = hidden || collapsed.has(node.id);
    node.children.forEach((child) =>
      emitNode(child, depth + 1, childHidden, node.id),
    );
  };

  forest.forEach((root) => emitNode(root, 0, false, null));

  return { entries, flatTasks };
}
