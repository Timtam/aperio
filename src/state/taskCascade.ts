import type { Task, TaskStatus } from '../api/types';

/**
 * Status coupling between parent tasks and their subtasks
 * (DESIGN.md §9.1 follow-up).
 *
 * Two directions:
 *
 * **Cascade up** — every time a child's status changes, the parent's
 * derived status is recomputed from the *combined* state of all its
 * children. The rules:
 *
 *   - any child in_progress              → parent in_progress
 *   - else any child completed + any open → parent in_progress (work
 *                                            started, not finished)
 *   - else all non-cancelled completed   → parent completed
 *   - else all children cancelled        → parent cancelled
 *   - else (only open / open+cancelled)  → parent open
 *
 * The recomputation propagates further up: if a parent's status
 * changes, its own parent is recomputed, all the way to the root.
 *
 * **Cascade down** — when a parent's status flips to completed or
 * cancelled, descendants follow. Respect prior decisions:
 *
 *   - parent → completed: every descendant that is *not already*
 *     cancelled becomes completed. Cancelled descendants stay
 *     cancelled (the user explicitly dropped them).
 *   - parent → cancelled: every descendant that is *not already*
 *     completed becomes cancelled. Completed descendants stay
 *     completed (the user already finished that work).
 *   - parent → open / in_progress: no cascade down. The child
 *     statuses keep their own; the cascade-up rule will re-derive
 *     the parent again on the next child change.
 *
 * The planner is pure: it takes the latest snapshot of tasks plus a
 * description of the desired change, and returns a flat list of
 * `{taskId, status}` writes the caller must apply in order. The
 * caller (a React hook) decides how to persist them — typically by
 * calling `update_task` once per write.
 */

export interface StatusWrite {
  taskId: string;
  status: TaskStatus;
}

/**
 * Derive a parent's status from the statuses of its children.
 * Returns `null` when the parent has no children (no derivation
 * possible — caller keeps the parent's existing status).
 */
export function deriveStatusFromChildren(
  children: Task[],
): TaskStatus | null {
  if (children.length === 0) return null;

  let hasInProgress = false;
  let hasOpen = false;
  let hasCompleted = false;
  // We don't track cancelled explicitly: "all cancelled" falls out
  // of "no open, no completed, no in_progress, at least one child"
  // below, and a single cancelled child mixed with open ones still
  // leaves the parent "open" by the same rule.

  for (const c of children) {
    switch (c.status) {
      case 'in_progress':
        hasInProgress = true;
        break;
      case 'open':
        hasOpen = true;
        break;
      case 'completed':
        hasCompleted = true;
        break;
      case 'cancelled':
        // intentionally not tracked — see comment above.
        break;
    }
  }

  if (hasInProgress) return 'in_progress';
  // Some progress made but not finished: at least one done plus at
  // least one still to do reads as "in progress" even if nobody is
  // *actively* working on it right now.
  if (hasCompleted && hasOpen) return 'in_progress';
  if (!hasOpen) {
    // All children are in a terminal state (completed or cancelled).
    if (!hasCompleted) return 'cancelled';
    // Mix of completed + cancelled, no open: the non-cancelled work
    // is done, so the parent is done. (Cancelled subtasks don't keep
    // the parent unfinished — the user explicitly walked away from
    // them.)
    return 'completed';
  }
  // Has open, no completed, no in_progress → still untouched.
  // `hasCancelled` may be true (one or more dropped), but if no
  // completed and no in_progress and at least one open, the parent
  // is "open" — work hasn't really started yet, just some intent
  // was dropped.
  return 'open';
}

/**
 * Plan all status writes for a single root change.
 *
 * `taskId` changes to `newStatus`. Descendants follow per the
 * cascade-down rule; ancestors are recomputed per the cascade-up
 * rule, recursing all the way to the root.
 *
 * The returned list starts with the root change itself, then the
 * descendants, then the ancestors from nearest to furthest. Idempotent
 * writes (status already matches) are omitted.
 */
export function planStatusCascade(
  taskId: string,
  newStatus: TaskStatus,
  allTasks: Task[],
): StatusWrite[] {
  const writes: StatusWrite[] = [];
  // `overrides` is the running snapshot of statuses as the cascade
  // computes them — used so the up-recompute "sees" the down-cascade
  // changes we've already decided.
  const overrides = new Map<string, TaskStatus>();
  const byId = new Map(allTasks.map((t) => [t.id, t]));
  const childrenByParent = buildChildrenIndex(allTasks);

  const statusOf = (id: string): TaskStatus | undefined =>
    overrides.has(id) ? overrides.get(id) : byId.get(id)?.status;

  // Root: skip if already that status.
  const rootCurrent = byId.get(taskId)?.status;
  if (rootCurrent !== newStatus) {
    writes.push({ taskId, status: newStatus });
    overrides.set(taskId, newStatus);
  }

  // Cascade down for terminal-state moves.
  if (newStatus === 'completed' || newStatus === 'cancelled') {
    const target = newStatus;
    // Respect prior decisions: the opposite terminal stays. Already-
    // target descendants are no-op.
    const oppositeTerminal: TaskStatus =
      target === 'completed' ? 'cancelled' : 'completed';
    const stack: string[] = [taskId];
    while (stack.length > 0) {
      const id = stack.pop()!;
      const kids = childrenByParent.get(id) ?? [];
      for (const kid of kids) {
        const kidStatus = statusOf(kid.id);
        if (kidStatus === oppositeTerminal) continue;
        if (kidStatus === target) {
          stack.push(kid.id);
          continue;
        }
        writes.push({ taskId: kid.id, status: target });
        overrides.set(kid.id, target);
        stack.push(kid.id);
      }
    }
  }

  // Cascade up: walk parents, recomputing each against the latest
  // sibling state (with overrides applied).
  let current = byId.get(taskId);
  while (current?.parent_id) {
    const parentId = current.parent_id;
    const parent = byId.get(parentId);
    if (!parent) break; // orphan — stop
    const siblings = childrenByParent.get(parentId) ?? [];
    // Apply overrides so the recompute sees the freshly cascaded
    // statuses, not the stale ones from `allTasks`.
    const effective: Task[] = siblings.map((s) => {
      const ov = overrides.get(s.id);
      return ov ? { ...s, status: ov } : s;
    });
    const derived = deriveStatusFromChildren(effective);
    if (derived === null) break;
    const parentEffective = statusOf(parentId);
    if (parentEffective === derived) break; // no change → no further propagation
    writes.push({ taskId: parentId, status: derived });
    overrides.set(parentId, derived);
    current = parent;
  }

  return writes;
}

/**
 * Recompute ancestors after a non-status mutation (subtask created,
 * subtask deleted). Identical to the up-half of `planStatusCascade`,
 * but starts directly from the parent — there is no root status
 * change to write.
 */
export function planAncestorRecompute(
  parentId: string,
  allTasks: Task[],
): StatusWrite[] {
  const writes: StatusWrite[] = [];
  const overrides = new Map<string, TaskStatus>();
  const byId = new Map(allTasks.map((t) => [t.id, t]));
  const childrenByParent = buildChildrenIndex(allTasks);

  let currentId: string | undefined = parentId;
  while (currentId) {
    const current = byId.get(currentId);
    if (!current) break;
    const siblings = childrenByParent.get(currentId) ?? [];
    const effective: Task[] = siblings.map((s) => {
      const ov = overrides.get(s.id);
      return ov ? { ...s, status: ov } : s;
    });
    const derived = deriveStatusFromChildren(effective);
    if (derived === null) break;
    const effectiveCurrent = overrides.get(currentId) ?? current.status;
    if (effectiveCurrent !== derived) {
      writes.push({ taskId: currentId, status: derived });
      overrides.set(currentId, derived);
    }
    currentId = current.parent_id ?? undefined;
  }

  return writes;
}

function buildChildrenIndex(all: Task[]): Map<string, Task[]> {
  const map = new Map<string, Task[]>();
  for (const t of all) {
    if (!t.parent_id) continue;
    const bucket = map.get(t.parent_id) ?? [];
    bucket.push(t);
    map.set(t.parent_id, bucket);
  }
  return map;
}
