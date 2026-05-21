import { useEffect, useRef } from 'react';

import {
  readFiredDayKey,
  shouldFireToday,
  useCurrentDayKey,
  writeFiredDayKey,
} from '../hooks/useCurrentDayKey';
import { useDialogState } from '../state/DialogState';
import { useTaskCascadeEnabled } from '../state/TaskCascadeProvider';
import { useTasks } from '../state/useTasks';
import { filterOverdue, isCurrentlySnoozed } from './MissedTasksDialog';

/**
 * Day-start gate that opens the missed-tasks dialog when there is at
 * least one task with a `deadline_date` strictly before today that is
 * still open or in_progress.
 *
 * Re-fires when the local date changes — the same `useCurrentDayKey`
 * +`shouldFireToday` pattern the CarryOver and DeadlinePin checkers
 * use. A persistent `firedDayKey` in localStorage prevents re-firing
 * for a day the user already dismissed.
 *
 * Snooze ("Später erinnern") bails without storing — the next poller
 * tick re-runs the gate once the four-hour suppress flag expires.
 *
 * Renders nothing; the dialog itself lives in DialogHost via
 * `DialogState.openMissedTasks()`.
 */
export function MissedTasksChecker() {
  const { tasks, loading } = useTasks();
  const { mode: dialogMode, openMissedTasks } = useDialogState();
  const { dayStartTrigger, hydrating } = useTaskCascadeEnabled();
  const todayKey = useCurrentDayKey();
  const firedRef = useRef<string | null>(readFiredDayKey('missedTasks'));

  useEffect(() => {
    if (loading || hydrating) return;
    if (!shouldFireToday(dayStartTrigger, firedRef.current, todayKey)) {
      return;
    }
    // Defer if another modal is up — pushing the missed-tasks dialog
    // would stack on top of an already-open editor / carry-over.
    // The minute poller will retry.
    if (dialogMode.kind !== 'none') return;
    // Snooze respects the user's "Später erinnern" choice. Don't
    // mark fired — next tick will run the gate again once the
    // snooze expires.
    if (isCurrentlySnoozed()) return;

    const overdue = filterOverdue(tasks);
    firedRef.current = todayKey;
    writeFiredDayKey('missedTasks', todayKey);
    if (overdue.length === 0) return;
    openMissedTasks();
  }, [
    loading,
    hydrating,
    tasks,
    dayStartTrigger,
    todayKey,
    dialogMode.kind,
    openMissedTasks,
  ]);

  return null;
}
