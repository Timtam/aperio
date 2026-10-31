import {
  familyNotAllSentence,
  failureOfWrites,
  planAncestorRecompute,
  planStatusCascade,
  selfAssignOnStatusChange,
  todayIsoKey,
  writeErrorMessage,
  writeStatusLanded,
} from '@aperio/shared';
import type { StatusWrite, Task, TaskList, TaskStatus } from '@aperio/shared';

import { updateTask } from '../api/client';
import { logLine } from '../api/logs';
import { currentUserForList } from './currentUser';
import {
  canStoreInProgress,
  effectiveForList,
  nextCheckoffStatus,
  readTaskBehaviour,
} from './taskBehaviour';

/** Apply each planned status write via update_task against `snapshot` (the task
 *  set the planner saw). Preserves an existing completion time, stamps a fresh
 *  one only on a newly-completed task, and honours the planner's auto-date
 *  companion (`scheduledDate !== undefined` = overwrite, else leave). When
 *  `autoSelfAssign` is on, also self-assigns me on a →in_progress/→completed of
 *  an unassigned task and drops only me on →open (shared lists with an identity).
 *  Shared by the check-off cascade, the ancestor recompute, and the editor
 *  cascade. */
export async function applyStatusWrites(
  writes: StatusWrite[],
  snapshot: Task[],
  autoSelfAssign: boolean,
  since?: Map<string, Task>,
): Promise<void> {
  const { failures, landed } = await tryStatusWrites(writes, snapshot, autoSelfAssign, since);
  if (failures.length > 0) {
    throw failureOfWrites(
      'cascade',
      failures.map((failure) => failure.err),
      landed,
    );
  }
}

/** [`applyStatusWrites`]'s writes, every one attempted: the failures as they
 *  came, by the task each write was planned for, and whether any write
 *  landed. `since`: rows written after the plan, by the id it names them
 *  under — a task a list change moved has a new id there (review round 20 of
 *  #128). */
async function tryStatusWrites(
  writes: StatusWrite[],
  snapshot: Task[],
  autoSelfAssign: boolean,
  since: Map<string, Task> = new Map(),
): Promise<{ failures: { taskId: string; err: unknown }[]; landed: boolean }> {
  const failures: { taskId: string; err: unknown }[] = [];
  let landed = false;
  if (writes.length === 0) return { failures, landed };
  const byId = new Map(snapshot.map((t) => [t.id, t]));
  const nowIso = new Date().toISOString();
  // The first rejection no longer ends the round. A cascade is N separate
  // provider writes with no transaction behind them, so stopping early left an
  // arbitrary prefix applied and the rest not — and which prefix depended on
  // nothing the user could see. Every write is attempted; the first error is
  // kept and thrown at the end, so each caller's existing `catch` still
  // announces exactly as before — but where another write landed, it never
  // says "nothing was changed" (decision 277). Mirrors the desktop.
  for (const w of writes) {
    const target = since.get(w.taskId) ?? byId.get(w.taskId);
    if (target == null) continue;
    // "me" is resolved per the task's list (session-cached); a list with no
    // identity yields null → the helper no-ops, as does the setting being off.
    const me = autoSelfAssign ? await currentUserForList(target.list_id) : null;
    const nextAssignees = selfAssignOnStatusChange(
      w.status,
      target.assignees,
      me,
      autoSelfAssign,
    );
    try {
      await updateTask({
        ...target,
        status: w.status,
        completed_at: w.status === 'completed' ? (target.completed_at ?? nowIso) : null,
        assignees: nextAssignees ?? target.assignees,
        scheduled_date:
          w.scheduledDate !== undefined ? w.scheduledDate : target.scheduled_date,
      });
      landed = true;
    } catch (err) {
      failures.push({ taskId: w.taskId, err });
    }
  }
  return { failures, landed };
}

/**
 * A check-off whose task changed — its save stood in part with its status —
 * while part of its family did not (review round 20 of #128): said as the
 * task's own sentence, then the family's, by [`checkOffErrorMessage`].
 */
export class CheckOffFailure {
  constructor(
    readonly task: unknown,
    readonly family: readonly unknown[],
  ) {}
}

/** What a failed check-off or status change says: the desktop's
 *  `useTaskStatusActions` sentences. */
export function checkOffErrorMessage(
  err: unknown,
  t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  if (err instanceof CheckOffFailure) {
    const sentence = writeErrorMessage(err.task, t);
    const family = familyNotAllSentence(err.family, t);
    return family ? `${sentence} ${family}` : sentence;
  }
  return writeErrorMessage(err, t);
}

/** A check-off's writes, `task` first: where its own save stood in part with
 *  its status, what of the family failed goes with it, as on the desktop. */
async function applyCheckOff(
  task: Task,
  writes: StatusWrite[],
  snapshot: Task[],
  autoSelfAssign: boolean,
): Promise<void> {
  const { failures, landed } = await tryStatusWrites(writes, snapshot, autoSelfAssign);
  if (failures.length === 0) return;
  const own = failures.find((failure) => failure.taskId === task.id);
  if (own && writeStatusLanded(own.err)) {
    const rest = failures.filter((failure) => failure !== own);
    throw rest.length > 0
      ? new CheckOffFailure(
          own.err,
          rest.map((failure) => failure.err),
        )
      : own.err;
  }
  throw failureOfWrites(
    'cascade',
    failures.map((failure) => failure.err),
    landed,
  );
}

/**
 * Recompute a parent's (and further ancestors') status after a subtask was
 * created or deleted, honouring the synced cascade-coupling + auto-date knobs —
 * the mobile twin of the desktop TaskDialog's applyAncestorWrites. `snapshot`
 * must already reflect the mutation (the created task present / the deleted task
 * removed). A no-op when coupling is off.
 */
export async function recomputeAncestors(
  parentId: string,
  snapshot: Task[],
): Promise<void> {
  const behaviour = await readTaskBehaviour();
  // Resolve cascade/auto-date for the parent's OWN list (subtasks stay in it),
  // so a per-list override applies; fall back to globals if the parent is gone.
  const parent = snapshot.find((tk) => tk.id === parentId);
  const eff = parent
    ? effectiveForList(behaviour, parent.list_id)
    : { cascade: behaviour.cascadeEnabled, autoDate: behaviour.autoDate };
  if (!eff.cascade) return;
  const writes = planAncestorRecompute(parentId, snapshot, {
    cascadeEnabled: eff.cascade,
    ...(eff.autoDate ? { todayKey: todayIsoKey() } : {}),
  });
  await applyStatusWrites(writes, snapshot, behaviour.autoSelfAssign);
}

/** All descendants of `parentId` (children, grandchildren, …) in `all` — the
 *  mobile twin of the desktop TaskDialog's collectDescendants. */
export function collectDescendants(parentId: string, all: Task[]): Task[] {
  const out: Task[] = [];
  const stack: string[] = [parentId];
  // Visit every task at most once — external providers can deliver a
  // parent cycle, which would otherwise loop (and grow `out`) forever.
  const seen = new Set<string>([parentId]);
  while (stack.length > 0) {
    const id = stack.pop();
    if (id == null) continue;
    for (const tk of all) {
      if (tk.parent_id === id && !seen.has(tk.id)) {
        seen.add(tk.id);
        out.push(tk);
        stack.push(tk.id);
      }
    }
  }
  return out;
}

/**
 * Cascade a status change made via the task EDITOR to the family — the mobile
 * twin of the desktop TaskDialog's planStatusCascade with the ROOT write
 * FILTERED OUT (the editor already wrote the root with its full field set, so
 * re-applying it from the snapshot would clobber the other edited fields).
 * `snapshot` MUST reflect the root's NEW status (and any post-edit list moves)
 * so the up/down cascade reads a coherent state. Honours the per-list cascade +
 * auto-date knobs, like the check-off path.
 */
export async function cascadeEditorStatus(
  taskId: string,
  newStatus: TaskStatus,
  listId: string,
  list: TaskList | undefined,
  snapshot: Task[],
  since?: Map<string, Task>,
): Promise<void> {
  const { writes, autoSelfAssign } = await planEditorStatus(
    taskId,
    newStatus,
    listId,
    list,
    snapshot,
  );
  await applyStatusWrites(writes, snapshot, autoSelfAssign, since);
}

/**
 * [`cascadeEditorStatus`] after a root whose save stood in part with its
 * status (review rounds 17 and 18 of #128): every write attempted, the
 * failures as they came, for the editor's sentence — the desktop's
 * `followFamily`.
 */
export async function followEditorStatus(
  taskId: string,
  newStatus: TaskStatus,
  listId: string,
  list: TaskList | undefined,
  snapshot: Task[],
): Promise<unknown[]> {
  const { writes, autoSelfAssign } = await planEditorStatus(
    taskId,
    newStatus,
    listId,
    list,
    snapshot,
  );
  return (await tryStatusWrites(writes, snapshot, autoSelfAssign)).failures.map(
    (failure) => failure.err,
  );
}

/** The family's writes an editor's status change makes, the root's own left
 *  out, with the self-assign knob. */
async function planEditorStatus(
  taskId: string,
  newStatus: TaskStatus,
  listId: string,
  list: TaskList | undefined,
  snapshot: Task[],
): Promise<{ writes: StatusWrite[]; autoSelfAssign: boolean }> {
  const behaviour = await readTaskBehaviour();
  const canInProgress = canStoreInProgress(list);
  const eff = effectiveForList(behaviour, listId);
  const writes = planStatusCascade(taskId, newStatus, snapshot, {
    cascadeEnabled: eff.cascade,
    ...(eff.autoDate && canInProgress ? { todayKey: todayIsoKey() } : {}),
  }).filter((w) => w.taskId !== taskId);
  return { writes, autoSelfAssign: behaviour.autoSelfAssign };
}

// The one shared check-off path for every task surface (TasksScreen + the
// WeekScreen calendar chips) — the mobile twin of the desktop
// useTaskStatusToggle. It reads the synced task-behaviour prefs fresh on each
// call (so a knob changed in Settings or on another device takes effect
// immediately), picks the next status per the check-off mode, plans the status
// cascade (honouring the coupling + auto-date knobs), and applies every write.
// The caller refetches afterwards and announces using the returned new status.

/**
 * Check off `task`: compute its next status (honouring check-off mode +
 * `supports_in_progress`), plan the cascade (status coupling + auto-date), and
 * apply every resulting write via update_task. Returns the task's NEW status
 * (for the caller's announce), or `null` when nothing changed.
 *
 * `list` is the task's owning list (for the `supports_in_progress` capability);
 * `allTasks` is the full snapshot the planner walks for parents/children.
 */
export async function applyTaskToggle(
  task: Task,
  list: TaskList | undefined,
  allTasks: Task[],
): Promise<TaskStatus | null> {
  // The moment of the tap, for measuring how long the write takes to leave:
  // the host logs the provider request, this line the start (TODO B13, 218).
  void logLine('info', 'task check-off: started');
  const behaviour = await readTaskBehaviour();
  const canInProgress = canStoreInProgress(list);
  const nextStatus = nextCheckoffStatus(task.status, behaviour.checkoffMode, canInProgress);
  if (nextStatus === task.status) return null;
  // Cascade + auto-date resolve PER-LIST (override per field, else global), so a
  // list flagged differently behaves so here too — matching the desktop. The
  // check-off MODE stays global (it's not per-list).
  const eff = effectiveForList(behaviour, task.list_id);
  const writes = planStatusCascade(task.id, nextStatus, allTasks, {
    cascadeEnabled: eff.cascade,
    // Auto-date pins a dateless task to today as it enters in_progress (the
    // planner only applies it to such writes). Skip it where the provider can't
    // store in_progress — nothing would persist. Off → omit todayKey entirely.
    ...(eff.autoDate && canInProgress ? { todayKey: todayIsoKey() } : {}),
  });
  if (writes.length === 0) return null;
  await applyCheckOff(task, writes, allTasks, behaviour.autoSelfAssign);
  return nextStatus;
}

/**
 * Set `task` to a SPECIFIC status (not a toggle) and cascade — the mobile twin
 * of the desktop useTaskStatusActions `set`. Used where a surface knows the
 * exact target state (e.g. the day-start review's "Mark done"). Reads the synced
 * task-behaviour fresh, resolves cascade/auto-date PER-LIST, plans the status
 * cascade, and applies every write. A no-op when the planner emits nothing.
 *
 * `list` is the task's owning list (for `supports_in_progress`); `allTasks` is
 * the snapshot the planner walks. The caller refetches + announces afterwards.
 * Returns the number of OTHER tasks the cascade also touched (writes beyond the
 * target itself) so the caller can append the canonical "N related tasks also
 * updated" suffix, matching the desktop status action.
 */
export async function setTaskStatusTo(
  task: Task,
  status: TaskStatus,
  list: TaskList | undefined,
  allTasks: Task[],
): Promise<number> {
  void logLine('info', `task status change to ${status}: started`);
  const behaviour = await readTaskBehaviour();
  const canInProgress = canStoreInProgress(list);
  const eff = effectiveForList(behaviour, task.list_id);
  const writes = planStatusCascade(task.id, status, allTasks, {
    cascadeEnabled: eff.cascade,
    // Auto-date only pins a dateless task entering in_progress (the planner
    // applies it to such writes only); skip where the provider can't store it.
    ...(eff.autoDate && canInProgress ? { todayKey: todayIsoKey() } : {}),
  });
  await applyCheckOff(task, writes, allTasks, behaviour.autoSelfAssign);
  return Math.max(0, writes.length - 1);
}

/** The announce message for a check-off's resulting status. */
export function statusAnnounce(
  t: (key: string, vars?: Record<string, unknown>) => string,
  status: TaskStatus,
  title: string,
): string {
  switch (status) {
    case 'completed':
      return t('mobile.completed', { title });
    case 'in_progress':
      return t('views.tasks.inProgressAnnounce', { title });
    case 'open':
    default:
      return t('mobile.reopened', { title });
  }
}
