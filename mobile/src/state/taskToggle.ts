import { planStatusCascade, todayIsoKey } from '@aperio/shared';
import type { Task, TaskList, TaskStatus } from '@aperio/shared';

import { updateTask } from '../api/client';
import { canStoreInProgress, nextCheckoffStatus, readTaskBehaviour } from './taskBehaviour';

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
  const behaviour = await readTaskBehaviour();
  const canInProgress = canStoreInProgress(list);
  const nextStatus = nextCheckoffStatus(task.status, behaviour.checkoffMode, canInProgress);
  if (nextStatus === task.status) return null;
  const writes = planStatusCascade(task.id, nextStatus, allTasks, {
    cascadeEnabled: behaviour.cascadeEnabled,
    // Auto-date pins a dateless task to today as it enters in_progress (the
    // planner only applies it to such writes). Skip it where the provider can't
    // store in_progress — nothing would persist. Off → omit todayKey entirely.
    ...(behaviour.autoDate && canInProgress ? { todayKey: todayIsoKey() } : {}),
  });
  if (writes.length === 0) return null;
  const byId = new Map(allTasks.map((t) => [t.id, t]));
  const nowIso = new Date().toISOString();
  for (const w of writes) {
    const target = byId.get(w.taskId);
    if (target == null) continue;
    await updateTask({
      ...target,
      status: w.status,
      // Preserve an existing completion time; stamp a fresh one only when this
      // write newly completes the task. `undefined` scheduledDate means "leave
      // the date alone"; a string is the auto-date companion write.
      completed_at: w.status === 'completed' ? (target.completed_at ?? nowIso) : null,
      scheduled_date:
        w.scheduledDate !== undefined ? w.scheduledDate : target.scheduled_date,
    });
  }
  return nextStatus;
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
