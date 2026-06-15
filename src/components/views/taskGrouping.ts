import type { Section, Task } from '../../api/types';
import { priorityRank } from '../../intl/taskStatus';

/** Sentinel id of the synthetic "Done (N)" group row. */
export const DONE_GROUP_ID = '__aperio_done_group__';
/** Sentinel id of the synthetic "Backlog" group row. */
export const BACKLOG_GROUP_ID = '__aperio_backlog_group__';

/** Metadata carried by a group-header row (Backlog / a list / a section /
 *  the synthetic Done group). When `group` is set on an {@link Entry}, the
 *  row is a collapsible header rather than a real task. */
export interface GroupMeta {
  kind: 'backlog' | 'list' | 'section' | 'done';
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
    scheduled_date: null,
    scheduled_time: null,
    deadline_date: null,
    deadline_time: null,
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

export function buildEntries(
  tasks: Task[],
  taskListById: Map<string, { name: string }>,
  t: (key: string, vars?: Record<string, unknown>) => string,
  collapsed: Set<string>,
  sectionsByList: Record<string, Section[]>,
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

  // Completed top-level tasks (with their subtree) collapse into a single
  // "Done (N)" group; the active groups show only open work. A completed
  // *subtask* under an open parent stays inline.
  const doneTopLevel: Task[] = [];
  const openTopLevel: Task[] = [];
  topLevel.forEach((task) => {
    if (task.status === 'completed') doneTopLevel.push(task);
    else openTopLevel.push(task);
  });

  // High priority floats up, low sinks. Stable sort keeps existing order as
  // the tiebreaker within a band. Every downstream bucket is filled by
  // walking `openTopLevel` in order, so this one sort is enough.
  openTopLevel.sort(
    (a, b) => priorityRank(a.priority) - priorityRank(b.priority),
  );

  // Backlog (no planned work day) vs the per-list groups (scheduled).
  const backlog: Task[] = [];
  const byList = new Map<string, Task[]>();
  openTopLevel.forEach((task) => {
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
          title: section.name,
          meta: { kind: 'section', sectionId: section.id, listId, section },
          children: secTasks.map((task) => ({ t: 'task', task }) as GNode),
        });
      });
    return out;
  };

  const forest: GNode[] = [];

  // Backlog → list → section. Grouping the backlog (not just the scheduled
  // tasks) is what makes e.g. a Vikunja project's buckets visible even when
  // nothing is scheduled.
  if (backlog.length > 0) {
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
        title: nameOf(listId),
        meta: { kind: 'list', listId },
        children: listChildren(listId, items, `bl:${listId}`),
      }));
    forest.push({
      t: 'group',
      id: BACKLOG_GROUP_ID,
      title: t('views.tasks.backlog'),
      meta: { kind: 'backlog' },
      children: listNodes,
    });
  }

  // Scheduled per-list groups, sorted by list name.
  Array.from(byList.entries())
    .sort(([a], [b]) => byName(a, b))
    .forEach(([listId, items]) => {
      forest.push({
        t: 'group',
        id: `grp:sc:list:${listId}`,
        title: nameOf(listId),
        meta: { kind: 'list', listId },
        children: listChildren(listId, items, `sc:${listId}`),
      });
    });

  // Done group last, most-recently-completed first.
  if (doneTopLevel.length > 0) {
    doneTopLevel.sort((a, b) =>
      (b.completed_at ?? '').localeCompare(a.completed_at ?? ''),
    );
    forest.push({
      t: 'group',
      id: DONE_GROUP_ID,
      title: t('views.tasks.done', { count: doneTopLevel.length }),
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
