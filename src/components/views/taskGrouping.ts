import type { Section, Task } from '../../api/types';
import { priorityRank } from '../../intl/taskStatus';

/** Sentinel id of the synthetic "Done (N)" parent row. It behaves like a
 *  collapsible parent treeitem whose children are the completed tasks, so
 *  it reuses the tree's keyboard model (Arrow/Enter/Space) instead of
 *  being a foreign tab-stop. The TaskView special-cases this id. */
export const DONE_GROUP_ID = '__aperio_done_group__';

/** One row in the flattened task tree the TaskView renders. */
export type Entry =
  | {
      kind: 'separator';
      label: string;
      level?: number;
      /** Set on a *section* sub-header — carries the section id so the
       *  header can tint to the section's color and offer a color
       *  action. Absent on the backlog / list headers. */
      sectionId?: string;
      /** The list this section belongs to — lets the header gate its
       *  color action to local lists (external sections are read-only). */
      listId?: string;
    }
  | {
      kind: 'task';
      task: Task;
      listName: string;
      /** Position in `flatTasks` — what `focusIndex` indexes into. */
      index: number;
      /** 0 for top-level, 1+ for nested under a parent. */
      depth: number;
      /** Set when this task has at least one child in the list. */
      hasChildren: boolean;
      /** True when the parent row above is collapsed (so the
       *  caller knows to skip this child during rendering). */
      hidden: boolean;
    };

export function buildEntries(
  tasks: Task[],
  taskListById: Map<string, { name: string }>,
  t: (key: string, vars?: Record<string, unknown>) => string,
  collapsed: Set<string>,
  sectionsByList: Record<string, Section[]>,
): { entries: Entry[]; flatTasks: Task[] } {
  // Bucket children under their parent so the depth-first walk
  // below has O(1) lookup. Tasks whose parent_id points at a
  // missing row become orphans — surface them at top level rather
  // than swallowing them silently.
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

  // Completed top-level tasks (with their whole subtree) leave the
  // active groups and collapse into a single "Done (N)" footer — the
  // active list shows only open work. A *completed subtask* under an
  // open parent stays inline (struck-through, in context); only the
  // top-level placement is diverted here.
  const doneTopLevel: Task[] = [];
  const openTopLevel: Task[] = [];
  topLevel.forEach((task) => {
    if (task.status === 'completed') doneTopLevel.push(task);
    else openTopLevel.push(task);
  });

  // High priority floats to the top, low sinks to the bottom (medium in
  // between). Sorting once here is enough: every downstream bucket — backlog,
  // per-list, per-section, ungrouped — is filled by walking `openTopLevel` in
  // order, and `Array.prototype.sort` is stable, so the existing order stays
  // the tiebreaker within each priority band. (`doneTopLevel` keeps its own
  // completed-at order.)
  openTopLevel.sort(
    (a, b) => priorityRank(a.priority) - priorityRank(b.priority),
  );

  // Two top-level buckets for the OPEN tasks: backlog (no dates at all)
  // and the per-list groups. Children inherit their parent's bucket so
  // a subtask of a backlog task lives under it, not somewhere else.
  const backlog: Task[] = [];
  const byList = new Map<string, Task[]>();
  openTopLevel.forEach((task) => {
    // Backlog = no planned WORK day. A deadline-only task stays here (so it
    // can be scheduled onto a day) while also showing on its deadline day.
    if (!task.scheduled_date) {
      backlog.push(task);
      return;
    }
    const bucket = byList.get(task.list_id) ?? [];
    bucket.push(task);
    byList.set(task.list_id, bucket);
  });

  // Sort list groups by display name (stable, user-meaningful — the raw
  // list ids are UUIDs for local lists). Shared by the scheduled groups and
  // the backlog's per-list sub-grouping.
  const byName = (a: string, b: string) =>
    (taskListById.get(a)?.name ?? a).localeCompare(
      taskListById.get(b)?.name ?? b,
    );
  const sortedLists = Array.from(byList.entries()).sort(([a], [b]) =>
    byName(a, b),
  );

  const entries: Entry[] = [];
  const flatTasks: Task[] = [];

  // Depth-first emit. Hidden rows still join `flatTasks` so the
  // index space stays stable across collapse — but the renderer
  // skips them, and the keyboard nav effect clamps focus on
  // collapse so the user never lands inside a hidden node.
  const visit = (task: Task, depth: number, hidden: boolean) => {
    const children = childrenByParent.get(task.id) ?? [];
    const listName =
      taskListById.get(task.list_id)?.name ?? task.list_id;
    entries.push({
      kind: 'task',
      task,
      listName,
      index: flatTasks.length,
      depth,
      hasChildren: children.length > 0,
      hidden,
    });
    flatTasks.push(task);
    const childHidden = hidden || collapsed.has(task.id);
    children.forEach((child) => visit(child, depth + 1, childHidden));
  };

  // Emit a list's tasks grouped by section, each section sub-header at
  // `sectionLevel`. Ungrouped tasks (no/unknown section) lead with no header.
  // Shared by the scheduled per-list groups and the backlog (nested one level
  // deeper, under its per-list sub-headers).
  const emitListSections = (
    listId: string,
    items: Task[],
    sectionLevel: number,
  ) => {
    const sections = sectionsByList[listId] ?? [];
    if (sections.length === 0) {
      // Section-less backend (or not yet loaded) → flat under the list.
      items.forEach((task) => visit(task, 0, false));
      return;
    }
    // Subtasks follow their parent via `visit`, so only top-level placement
    // matters here.
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
    // Ungrouped tasks lead, with no sub-header (the fallback bucket).
    ungrouped.forEach((task) => visit(task, 0, false));
    // Then each non-empty section in its declared order, under a sub-header.
    [...sections]
      .sort((a, b) => a.order - b.order)
      .forEach((section) => {
        const secTasks = bySection.get(section.id);
        if (!secTasks || secTasks.length === 0) return;
        entries.push({
          kind: 'separator',
          label: section.name,
          level: sectionLevel,
          sectionId: section.id,
          listId,
        });
        secTasks.forEach((task) => visit(task, 0, false));
      });
  };

  if (backlog.length > 0) {
    entries.push({ kind: 'separator', label: t('views.tasks.backlog') });
    // Group the backlog by list → section too, so a list's sections (e.g. a
    // Vikunja project's To-Do / Doing / Done buckets) appear as groups even
    // when nothing is scheduled. Nested one level deeper than the scheduled
    // groups: Backlog (0) → list (1) → section (2).
    const backlogByList = new Map<string, Task[]>();
    backlog.forEach((task) => {
      const arr = backlogByList.get(task.list_id) ?? [];
      arr.push(task);
      backlogByList.set(task.list_id, arr);
    });
    Array.from(backlogByList.entries())
      .sort(([a], [b]) => byName(a, b))
      .forEach(([listId, items]) => {
        const name = taskListById.get(listId)?.name ?? listId;
        entries.push({ kind: 'separator', label: name, level: 1 });
        emitListSections(listId, items, 2);
      });
  }

  sortedLists.forEach(([listId, items]) => {
    const name = taskListById.get(listId)?.name ?? listId;
    entries.push({ kind: 'separator', label: name });
    emitListSections(listId, items, 1);
  });

  // "Done (N)" footer group — a single collapsible bucket for every
  // completed top-level task across all lists, most-recently-completed
  // first. Modelled as a *synthetic parent treeitem* (sentinel id) whose
  // children are the done tasks, so it slots straight into the tree's
  // keyboard model: Arrow navigates to it, Arrow-Right / Enter / Space
  // expand it, and its rows hide/show through the same `collapsed` set as
  // any subtree. Collapsed-by-default lives in that set (the caller seeds
  // it), keeping completed tasks out of the active list.
  if (doneTopLevel.length > 0) {
    doneTopLevel.sort((a, b) =>
      (b.completed_at ?? '').localeCompare(a.completed_at ?? ''),
    );
    // Re-parent the done tasks under the synthetic group so `visit`
    // emits them as its depth-1 children. Their own subtasks still nest
    // beneath them via the existing `childrenByParent` lookup.
    childrenByParent.set(DONE_GROUP_ID, doneTopLevel);
    const groupHeader: Task = {
      ...doneTopLevel[0],
      id: DONE_GROUP_ID,
      parent_id: null,
      title: t('views.tasks.done', { count: doneTopLevel.length }),
    };
    visit(groupHeader, 0, false);
  }

  return { entries, flatTasks };
}
