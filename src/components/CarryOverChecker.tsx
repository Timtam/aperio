import { useEffect, useRef } from 'react';

import { useDialogState } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { filterCarriedOver, isCarryOverSnoozed } from './CarryOverDialog';

/**
 * Mount-once gate that pushes the carry-over dialog when:
 *
 *   1. The task catalog has finished loading.
 *   2. There is at least one task with a `scheduled_date` strictly
 *      before today that is still open or in progress.
 *   3. The user hasn't snoozed the carry-over dialog within the
 *      last four hours.
 *
 * The component renders nothing; the dialog itself lives in
 * `DialogHost` and is reached via `openCarryOver()`.
 *
 * Stack interaction with {@link MissedTasksChecker}: both checkers
 * push onto the DialogState stack on mount. The checker mounted
 * *later* in App.tsx ends up on top of the stack, so the user sees
 * its dialog first. We mount missed-tasks AFTER carry-over so the
 * deadline-overdue prompt (more urgent) gets handled first; closing
 * it reveals the carry-over dialog underneath. Each checker has its
 * own snooze key so dismissing one doesn't silence the other.
 */
export function CarryOverChecker() {
  const { tasks, loading } = useTasks();
  const { openCarryOver } = useDialogState();
  const firedRef = useRef(false);

  useEffect(() => {
    if (firedRef.current) return;
    if (loading) return;
    if (isCarryOverSnoozed()) {
      // User already opted out for this window — treat as fired so
      // we don't re-evaluate on every refetch.
      firedRef.current = true;
      return;
    }
    const slipped = filterCarriedOver(tasks);
    if (slipped.length === 0) {
      firedRef.current = true;
      return;
    }
    firedRef.current = true;
    openCarryOver();
  }, [loading, tasks, openCarryOver]);

  return null;
}
