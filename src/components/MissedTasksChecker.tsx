import { useEffect, useRef } from 'react';

import { useDialogState } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { filterOverdue, isCurrentlySnoozed } from './MissedTasksDialog';

/**
 * Mount-once gate that opens the missed-tasks dialog when:
 *
 *   1. The task catalog has finished loading (no flash on first render
 *      while useTasks is still pending), and
 *   2. There is at least one overdue, not-completed task, and
 *   3. The user hasn't snoozed the dialog within the last 4 hours.
 *
 * Renders nothing; the dialog itself is hosted by DialogHost via
 * `DialogState.openMissedTasks()`.
 *
 * "Mount-once" is enforced with a ref so re-renders triggered by
 * useTasks re-fetches don't re-open the dialog after the user
 * dismissed it — the user opted out, leave them alone for the
 * session.
 */
export function MissedTasksChecker() {
  const { tasks, loading } = useTasks();
  const { openMissedTasks } = useDialogState();
  const firedRef = useRef(false);

  useEffect(() => {
    if (firedRef.current) return;
    if (loading) return;
    if (isCurrentlySnoozed()) {
      // The user already snoozed; consider it "fired" so we don't
      // re-evaluate on every refetch this session.
      firedRef.current = true;
      return;
    }
    const overdue = filterOverdue(tasks);
    if (overdue.length === 0) {
      firedRef.current = true;
      return;
    }
    firedRef.current = true;
    openMissedTasks();
  }, [loading, tasks, openMissedTasks]);

  return null;
}
