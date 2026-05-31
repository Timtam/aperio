import type { Section, Task } from '../../api/types';

/** One row in the flattened task tree the TaskView renders. */
export type Entry =
  | { kind: 'separator'; label: string; level?: number }
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

  // Two top-level buckets: backlog (no dates at all) and the
  // per-list groups. Children inherit their parent's bucket so a
  // subtask of a backlog task lives under it, not somewhere else.
  const backlog: Task[] = [];
  const byList = new Map<string, Task[]>();
  topLevel.forEach((task) => {
    if (!task.scheduled_date && !task.deadline_date) {
      backlog.push(task);
      return;
    }
    const bucket = byList.get(task.list_id) ?? [];
    bucket.push(task);
    byList.set(task.list_id, bucket);
  });

  const sortedLists = Array.from(byList.entries()).sort(([a], [b]) =>
    a.localeCompare(b),
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

  if (backlog.length > 0) {
    entries.push({ kind: 'separator', label: t('views.tasks.backlog') });
    backlog.forEach((task) => visit(task, 0, false));
  }

  sortedLists.forEach(([listId, items]) => {
    const name = taskListById.get(listId)?.name ?? listId;
    entries.push({ kind: 'separator', label: name });

    const sections = sectionsByList[listId] ?? [];
    if (sections.length === 0) {
      // Section-less backend (or not yet loaded) → flat under the list,
      // exactly the pre-sections shape.
      items.forEach((task) => visit(task, 0, false));
      return;
    }

    // Group the list's top-level tasks by section. Subtasks follow
    // their parent via `visit`, so only top-level placement matters.
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
    // Then each non-empty section in its declared order, under a
    // level-1 sub-header.
    [...sections]
      .sort((a, b) => a.order - b.order)
      .forEach((section) => {
        const secTasks = bySection.get(section.id);
        if (!secTasks || secTasks.length === 0) return;
        entries.push({ kind: 'separator', label: section.name, level: 1 });
        secTasks.forEach((task) => visit(task, 0, false));
      });
  });

  return { entries, flatTasks };
}
