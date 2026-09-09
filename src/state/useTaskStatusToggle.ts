import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage, selfAssignOnStatusChange } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { todayIsoKey } from '../intl/taskDay';
import { currentUserForList } from './currentUser';
import { useDialogState } from './dialogStateContext';
import { planStatusCascade, type StatusWrite } from './taskCascade';
import { useTaskCascadeEnabled } from './taskCascadeContext';
import { canStoreInProgress } from './taskMoves';
import { useTasks } from './useTasks';
import type { Task, TaskStatus } from '../api/types';

/**
 * Shared task-status mutations. Every task surface (TaskView, plus
 * the calendar chips in WeekView and DayView) needs:
 *
 *   - `toggle(task)` — Space-key contract from §9.4: flip between
 *     `open` and `completed`. Mirrors the visible ○/● marker.
 *   - `set(task, status)` — explicit assignment, used by the chip
 *     context menu's "Status > {Offen, In Arbeit, Erledigt,
 *     Abgebrochen}" submenu.
 *
 * Both routes go through `planStatusCascade`, so a status change on
 * a parent or child task ripples through the family per the rules
 * in `taskCascade.ts`:
 *
 *   - parent → completed cascades to non-cancelled descendants
 *   - parent → cancelled cascades to non-completed descendants
 *   - any child change recomputes the parent (and so on up the tree)
 *
 * SR users hear the cascade scope as a count appended to the focused
 * task's announce ("X erledigt. 4 weitere Aufgaben mit aktualisiert."),
 * so it's clear that flipping one row touched several.
 */

export interface TaskStatusActions {
  toggle: (task: Task) => Promise<void>;
  set: (task: Task, status: TaskStatus) => Promise<void>;
}

export function useTaskStatusActions(): TaskStatusActions {
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const { invalidateData } = useDialogState();
  // The cascade planner needs the latest snapshot of every task so
  // it can walk parents and siblings. `useTasks` returns the global
  // store, refreshed whenever `dataVersion` bumps. `taskListById` lets
  // us read the owning list's capabilities (e.g. whether it can store
  // the in_progress status at all).
  const { tasks, taskListById } = useTasks();
  // Honour two Settings → Tasks knobs PER LIST:
  //   - `cascade` (cascade-status-coupling): when off the planner
  //     degrades to a single-row write.
  //   - `autoDate`: when off the planner does NOT pin a started
  //     backlog task to today; we simply omit `todayKey` from the
  //     options. The cascade itself still runs normally.
  //
  // Per-task lookup via `effectiveForList(task.list_id)` so a user
  // who set "cascade off" for one specific list gets that respected
  // here without affecting the rest of the app. Parent and child
  // tasks live in the same list (invariant from #98), so the cascade
  // planner walking the tree all reads the same per-list setting.
  const { effectiveForList, checkoffMode, autoSelfAssign } =
    useTaskCascadeEnabled();

  const set = useCallback(
    async (task: Task, nextStatus: TaskStatus): Promise<void> => {
      if (task.status === nextStatus) return;
      const { cascade, autoDate } = effectiveForList(task.list_id);
      // Skip the auto-pin when the owning provider can't store
      // in_progress (e.g. Google Tasks / Todoist): the status reverts to
      // open on the next read, so silently moving the date to today would
      // be a surprise with nothing to show for it. The
      // explicit scheduling paths (drag onto a day, the plan dialog)
      // are untouched — they don't go through this cascade.
      const inProgressSticks = canStoreInProgress(taskListById.get(task.list_id));
      const writes = planStatusCascade(task.id, nextStatus, tasks, {
        cascadeEnabled: cascade,
        // Auto-date: a dateless task transitioning into in_progress
        // (either directly or because the up-cascade derived it from
        // a child) gets pinned to today, so the carry-over /
        // missed-tasks flow can locate it later. Opt-out via the
        // Settings → Tasks autoDate toggle — when off we omit
        // `todayKey` and the planner stops emitting the companion
        // scheduledDate field.
        ...(autoDate && inProgressSticks ? { todayKey: todayIsoKey() } : {}),
      });
      if (writes.length === 0) return;
      const outcome = await applyCascade(writes, tasks, autoSelfAssign);
      // ALWAYS, even on a failure: whatever landed before it is real, and a
      // view still showing the old state is a view that disagrees with the
      // store.
      invalidateData();

      // The focused row is its own case. If ITS write was rejected, the task
      // did not change status, so saying "completed" would be a lie — and a
      // lie is what the old `console.warn` amounted to by saying nothing at
      // all while the family changed underneath.
      const rootFailure = outcome.failed.find((f) => f.taskId === task.id);
      if (rootFailure) {
        announce(
          t('views.tasks.statusChangeFailed', {
            title: task.title,
            message: errorMessage(rootFailure.err),
          }),
        );
        return;
      }

      // Announce: focused task gets the usual status message; if additional
      // rows were touched, append what happened to them, so SR users know
      // they didn't change just one row — and know when some of them did not
      // take.
      const others = writes.filter((w) => w.taskId !== task.id).length;
      const othersFailed = outcome.failed.length;
      const base = announceFor(t, nextStatus, task.title);
      if (othersFailed > 0) {
        announce(
          `${base} ${t('views.tasks.cascadePartial', {
            applied: others - othersFailed,
            total: others,
            failed: othersFailed,
          })}`,
        );
      } else if (others > 0) {
        announce(`${base} ${t('views.tasks.cascadeSuffix', { count: others })}`);
      } else {
        announce(base);
      }
    },
    [
      announce,
      t,
      invalidateData,
      tasks,
      taskListById,
      effectiveForList,
      autoSelfAssign,
    ],
  );

  const toggle = useCallback(
    async (task: Task): Promise<void> => {
      const nextStatus: TaskStatus =
        checkoffMode === 'cycle'
          ? // Skip the in_progress step on providers that can't store it
            // (e.g. Google Tasks / Todoist): it would revert to open on
            // read-back, trapping the cycle at open so a check-off could
            // never reach completed. There the cycle is open → completed →
            // open. (Vikunja CAN store it, via percent_done.)
            nextCycleStatus(
              task.status,
              canStoreInProgress(taskListById.get(task.list_id)),
            )
          : // Default: flip between open and completed (anything not
            // already completed — open / in_progress / cancelled — becomes
            // completed; completed goes back to open).
            task.status === 'completed'
            ? 'open'
            : 'completed';
      await set(task, nextStatus);
    },
    [set, checkoffMode, taskListById],
  );

  return useMemo(() => ({ toggle, set }), [toggle, set]);
}

/**
 * Compatibility shim: existing call sites destructure a single
 * callable, not an `{ toggle, set }` object. This wrapper keeps that
 * shape so the migration can land in one commit without touching
 * every consumer at once.
 */
export function useTaskStatusToggle(): (task: Task) => Promise<void> {
  const { toggle } = useTaskStatusActions();
  return toggle;
}

/**
 * Three-state check-off cycle (Settings → Tasks → check-off mode = cycle):
 * `open → in_progress → completed → open`. A cancelled task re-enters the
 * cycle at `open` so a check-off un-cancels it rather than dead-ending.
 *
 * `canInProgress` (default `true`) drops the in_progress step for providers
 * that can't store it (e.g. Google Tasks / Todoist) — there the cycle is
 * `open → completed → open`, so a check-off isn't trapped at open by a
 * status that reverts on the next read.
 */
export function nextCycleStatus(
  current: TaskStatus,
  canInProgress = true,
): TaskStatus {
  switch (current) {
    case 'open':
      return canInProgress ? 'in_progress' : 'completed';
    case 'in_progress':
      return 'completed';
    case 'completed':
      return 'open';
    default:
      // cancelled (or any future state) → back into the cycle.
      return 'open';
  }
}

/**
 * Apply each StatusWrite by issuing an `update_task` call. The
 * snapshot is used to look up the unchanged fields of each task;
 * only `status` and `completed_at` differ. Writes execute serially
 * — a future Tauri-side batch command could collapse this into one
 * transaction, but for typical task counts the serial path keeps
 * the cascade local to one round-trip per row.
 */
/**
 * What actually reached the store.
 *
 * A cascade is N separate provider writes, and the third can fail while the
 * first two have already landed. There is no transaction to roll back — a
 * "rollback" would be two more writes that can fail in turn — so the honest
 * shape is best-effort plus a report: try every write, and say afterwards how
 * many made it.
 *
 * This used to be a bare `Promise<void>` that threw on the first failure, and
 * the caller answered with a `console.warn`. For a screen-reader user that is
 * the worst possible outcome: the row is checked off, some of the family has
 * changed underneath, and nothing at all is announced.
 */
interface CascadeOutcome {
  /** Task ids whose write reached the store. */
  applied: string[];
  /** Task ids whose write was rejected, with the first error seen. */
  failed: { taskId: string; err: unknown }[];
}

async function applyCascade(
  writes: StatusWrite[],
  snapshot: Task[],
  autoSelfAssign: boolean,
): Promise<CascadeOutcome> {
  const applied: string[] = [];
  const failed: CascadeOutcome['failed'] = [];
  const byId = new Map(snapshot.map((row) => [row.id, row]));
  for (const w of writes) {
    const target = byId.get(w.taskId);
    if (!target) continue;
    // Self-assignment (shared lists): assign me on →in_progress/→completed when
    // nobody owns the task, drop only me on →open. "me" is resolved per the
    // task's list (session-cached); a list with no identity yields null → the
    // helper no-ops, as does the setting being off.
    const me = autoSelfAssign
      ? await currentUserForList(target.list_id)
      : null;
    const nextAssignees = selfAssignOnStatusChange(
      w.status,
      target.assignees,
      me,
      autoSelfAssign,
    );
    // Every write is attempted, including the ones after a failure. Stopping
    // early would leave an arbitrary prefix applied and the rest not, with
    // nothing anywhere recording where it stopped.
    try {
      await invoke<Task>('update_task', {
        task: {
          ...target,
          status: w.status,
          completed_at:
            w.status === 'completed' ? new Date().toISOString() : null,
          assignees: nextAssignees ?? target.assignees,
          // Honour the planner's auto-date companion write: when a task
          // transitions into in_progress without a scheduled_date, the
          // planner pins it to today so the carry-over flow can find it
          // later. `undefined` means "no change", in which case we keep
          // the existing date.
          scheduled_date:
            w.scheduledDate !== undefined
              ? w.scheduledDate
              : target.scheduled_date,
        },
      });
      applied.push(w.taskId);
    } catch (err) {
      failed.push({ taskId: w.taskId, err });
    }
  }
  return { applied, failed };
}

function announceFor(
  t: (key: string, values?: Record<string, unknown>) => string,
  status: TaskStatus,
  title: string,
): string {
  switch (status) {
    case 'completed':
      return t('views.tasks.completedAnnounce', { title });
    case 'open':
      return t('views.tasks.reopenedAnnounce', { title });
    case 'in_progress':
      return t('views.tasks.inProgressAnnounce', { title });
    case 'cancelled':
      return t('views.tasks.cancelledAnnounce', { title });
  }
}
