import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/announcerContext';
import { todayIsoKey } from '../intl/taskDay';
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
  const { effectiveForList, checkoffMode } = useTaskCascadeEnabled();

  const set = useCallback(
    async (task: Task, nextStatus: TaskStatus): Promise<void> => {
      if (task.status === nextStatus) return;
      const { cascade, autoDate } = effectiveForList(task.list_id);
      // Skip the auto-pin when the owning provider can't store
      // in_progress (Google Tasks / Vikunja / Todoist): the status
      // reverts to open on the next read, so silently moving the date
      // to today would be a surprise with nothing to show for it. The
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
      try {
        await applyCascade(writes, tasks);
        invalidateData();
        // Announce: focused task gets the usual status message; if
        // additional rows were touched, append a count so SR users
        // know they didn't change just one row.
        const cascadeCount = writes.length - 1;
        const base = announceFor(t, nextStatus, task.title);
        announce(
          cascadeCount > 0
            ? `${base} ${t('views.tasks.cascadeSuffix', {
                count: cascadeCount,
              })}`
            : base,
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      }
    },
    [announce, t, invalidateData, tasks, taskListById, effectiveForList],
  );

  const toggle = useCallback(
    async (task: Task): Promise<void> => {
      const nextStatus: TaskStatus =
        checkoffMode === 'cycle'
          ? // Skip the in_progress step on providers that can't store it
            // (Google Tasks / Vikunja / Todoist): it would revert to open
            // on read-back, trapping the cycle at open so a check-off could
            // never reach completed. There the cycle is open → completed →
            // open.
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
 * that can't store it (Google Tasks / Vikunja / Todoist) — there the cycle
 * is `open → completed → open`, so a check-off isn't trapped at open by a
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
async function applyCascade(
  writes: StatusWrite[],
  snapshot: Task[],
): Promise<void> {
  const byId = new Map(snapshot.map((row) => [row.id, row]));
  for (const w of writes) {
    const target = byId.get(w.taskId);
    if (!target) continue;
    await invoke<Task>('update_task', {
      task: {
        ...target,
        status: w.status,
        completed_at:
          w.status === 'completed' ? new Date().toISOString() : null,
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
  }
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
