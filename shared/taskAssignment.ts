import type { TaskStatus, TaskUser } from './types';

/**
 * The new assignee list for a status transition, or `undefined` when nothing
 * should change. In shared lists where the adapter knows "me":
 *   - →`in_progress` / →`completed` on an UNASSIGNED task ⇒ `[me]`
 *   - →`open` while I'm an assignee                       ⇒ assignees minus me
 *
 * Only fires when `enabled` and a `me` identity exists for the task's list. The
 * reopen path removes ONLY me (a colleague's assignment survives), symmetric to
 * the auto-assign, which only acts on a task nobody owns. `cancelled` and
 * already-correct transitions are left untouched.
 */
export function selfAssignOnStatusChange(
  nextStatus: TaskStatus,
  assignees: TaskUser[],
  me: TaskUser | null,
  enabled: boolean,
): TaskUser[] | undefined {
  if (!enabled || !me) return undefined;
  const becameActive = nextStatus === 'in_progress' || nextStatus === 'completed';
  if (becameActive && assignees.length === 0) return [me];
  if (nextStatus === 'open' && assignees.some((a) => a.id === me.id)) {
    return assignees.filter((a) => a.id !== me.id);
  }
  return undefined;
}

/**
 * True when a task is "mine to act on": there's no identity, OR it's unassigned,
 * OR I'm one of the assignees. False ONLY when it's assigned to concrete OTHER
 * users and not me. Shared by the Done-counter split and the day-start ownership
 * filter (a colleague's task is neither counted as mine nor offered to me).
 */
export function isMineOrUnassigned(assignees: TaskUser[], me: TaskUser | null): boolean {
  return !me || assignees.length === 0 || assignees.some((a) => a.id === me.id);
}

/**
 * Classify a completed task for the "Done — N by me, M by others" split.
 * Personal lists (no identity) and unassigned tasks count as mine.
 */
export function classifyDoneByMe(assignees: TaskUser[], me: TaskUser | null): 'me' | 'other' {
  return isMineOrUnassigned(assignees, me) ? 'me' : 'other';
}
