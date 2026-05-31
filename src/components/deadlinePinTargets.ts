import type { Task } from '../api/types';
import { todayIsoKey } from '../intl/taskDay';

/**
 * Open / in_progress tasks whose deadline lands on today AND aren't
 * already pinned to today. The double check on `scheduled_date !==
 * today` keeps the batch idempotent across re-launches inside the
 * same calendar day.
 */
export function filterDeadlinePinTargets(tasks: Task[]): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.deadline_date !== today) return false;
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
    }
    if (task.scheduled_date === today) return false;
    return true;
  });
}
