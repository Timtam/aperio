import type { TaskList } from '../api/types';

export interface TaskListNode {
  list: TaskList;
  children: TaskListNode[];
  /** 0 for roots, +1 per level — the sidebar uses it for indentation. */
  depth: number;
}

/**
 * Build a parent→children forest from a flat task-list array using
 * `parent_id`. A list whose `parent_id` isn't present in `lists`
 * (different account, or a parent the user can't see) is promoted to a
 * root so nothing is dropped. Children preserve input order; flat
 * backends — where every `parent_id` is `null` — produce a depth-0
 * forest, exactly the pre-nesting shape.
 *
 * Callers pass an account-scoped subset (parent_id only ever refers to
 * a same-account list), so the forest never crosses account boundaries.
 */
export function buildTaskListForest(lists: TaskList[]): TaskListNode[] {
  const byId = new Map(lists.map((l) => [l.id, l]));
  const childrenOf = new Map<string, TaskList[]>();
  const roots: TaskList[] = [];
  for (const list of lists) {
    const parent = list.parent_id;
    if (parent && parent !== list.id && byId.has(parent)) {
      const arr = childrenOf.get(parent);
      if (arr) arr.push(list);
      else childrenOf.set(parent, [list]);
    } else {
      roots.push(list);
    }
  }
  const seen = new Set<string>();
  const build = (list: TaskList, depth: number): TaskListNode => {
    seen.add(list.id);
    const kids = (childrenOf.get(list.id) ?? []).filter((c) => !seen.has(c.id));
    return { list, depth, children: kids.map((c) => build(c, depth + 1)) };
  };
  const forest = roots.map((r) => build(r, 0));
  // Safety net: a list trapped in a parent cycle (corrupt data) is
  // never reached from a root — surface it as a depth-0 node rather
  // than dropping it silently.
  for (const list of lists) {
    if (!seen.has(list.id)) forest.push(build(list, 0));
  }
  return forest;
}
