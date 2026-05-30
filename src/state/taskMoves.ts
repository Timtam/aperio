// Capability-gating for task / project moves (TASKS-11).
//
// Pure predicates the UI consults before offering a move affordance —
// dragging a task to another section/project, or nesting one project
// under another. They centralise the rules so the sidebar, the task
// dialog and the task view all gate identically, and so the logic is
// unit-testable without a DOM.
//
// All predicates read the backend-stamped `TaskList.task_capabilities`.
// An absent capabilities block falls back to the cal-core-native
// default (flat lists, single-level subtasks, cross-list move) so a
// list from a pre-capabilities snapshot still behaves sensibly.

import type { TaskCapabilities, TaskList } from '../api/types';

/** cal-core-native defaults — mirror `TaskCapabilities::default()`. */
const DEFAULT_CAPS: TaskCapabilities = {
  nested_projects: false,
  subtasks: true,
  max_subtask_depth: null,
  sections: false,
  multiple_labels: false,
  task_recurrence: true,
  move_between_projects: true,
};

export function capabilitiesOf(list: TaskList | undefined): TaskCapabilities {
  return { ...DEFAULT_CAPS, ...(list?.task_capabilities ?? {}) };
}

/**
 * Can a task be moved out of `sourceList` into a different list?
 * Gated on the *source* adapter's `move_between_projects` — Todoist's
 * REST v2 can't change a task's project, so it declares `false` and we
 * lock the list picker for its tasks.
 */
export function canMoveTaskBetweenLists(
  sourceList: TaskList | undefined,
): boolean {
  return capabilitiesOf(sourceList).move_between_projects;
}

/** Can tasks in `list` be filed into a section (Vikunja bucket /
 *  Todoist section)? Gated on the list's `sections` capability. */
export function canAssignSection(list: TaskList | undefined): boolean {
  return capabilitiesOf(list).sections;
}

/** Can `list` host nested child projects / be reparented at all?
 *  Gated on its account's `nested_projects` capability. */
export function supportsNestedProjects(list: TaskList | undefined): boolean {
  return capabilitiesOf(list).nested_projects;
}

/**
 * Can `listId` be reparented under `newParentId` within `allLists`?
 *
 * Rules:
 *   - the moved list's adapter must support `nested_projects`;
 *   - parent and child must belong to the same account (you can't nest
 *     a Vikunja project under a Todoist one);
 *   - a list can't be its own parent;
 *   - the new parent must not be the list itself or any of its
 *     descendants — that would create a cycle.
 *
 * `newParentId === null` means "promote to top level" and is allowed
 * whenever the adapter supports nesting.
 */
export function canReparentList(
  listId: string,
  newParentId: string | null,
  allLists: TaskList[],
): boolean {
  const byId = new Map(allLists.map((l) => [l.id, l]));
  const list = byId.get(listId);
  if (!list) return false;
  if (!supportsNestedProjects(list)) return false;
  if (newParentId === null) return true;
  if (newParentId === listId) return false;

  const parent = byId.get(newParentId);
  if (!parent) return false;
  if (parent.account_id !== list.account_id) return false;

  // Walk up from the prospective parent; if we reach `listId` the move
  // would form a cycle. Bounded by a visited set against corrupt data.
  const seen = new Set<string>();
  let cursor: string | null = newParentId;
  while (cursor) {
    if (cursor === listId) return false;
    if (seen.has(cursor)) break;
    seen.add(cursor);
    cursor = byId.get(cursor)?.parent_id ?? null;
  }
  return true;
}

/**
 * The lists `listId` can be reparented under — every valid target for a
 * "move under…" menu / drop. Excludes itself, its current parent (a
 * no-op) and anything `canReparentList` rejects (cross-account, cycle,
 * non-nesting adapter). Returns [] when the adapter doesn't nest.
 */
export function reparentCandidates(
  listId: string,
  allLists: TaskList[],
): TaskList[] {
  const self = allLists.find((l) => l.id === listId);
  if (!self || !supportsNestedProjects(self)) return [];
  return allLists.filter(
    (l) =>
      l.id !== listId &&
      l.id !== self.parent_id &&
      canReparentList(listId, l.id, allLists),
  );
}
