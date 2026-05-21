import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import type { Task } from '../api/types';
import {
  readFiredDayKey,
  shouldFireToday,
  useCurrentDayKey,
  writeFiredDayKey,
} from '../hooks/useCurrentDayKey';
import { todayIsoKey } from '../intl/taskDay';
import { useDialogState } from '../state/DialogState';
import { useTaskCascadeEnabled } from '../state/TaskCascadeProvider';
import { useTasks } from '../state/useTasks';

/**
 * Mount-once silent pin: tasks whose `deadline_date` is today get
 * `scheduled_date` set to today, so they appear on today's calendar
 * surfaces. Implements the "by"-deadline auto-pin we agreed on when
 * specifying the new time-fields model.
 *
 * Pin condition:
 *   - `deadline_date == today`
 *   - status is open or in_progress (terminal states left alone)
 *   - `scheduled_date != today` (already there → no-op)
 *
 * Other deadline configurations are handled elsewhere:
 *   - `deadline_date < today` (missed) → MissedTasksDialog asks per row
 *   - `deadline_date > today` (future) → nothing yet; the task lives in
 *     backlog or its scheduled day until it slips into one of the
 *     other branches
 *
 * The pin deliberately leaves `scheduled_time` untouched. A user who
 * set `deadline_time = 14:30` meant "by 14:30", not "at 14:30 I will
 * work on this". The chip still surfaces at 14:30 in the timed lane
 * via `taskTimeOnDay`'s deadline-side path.
 *
 * Ordering vs. CarryOverChecker: this checker mounts AFTER it in
 * App.tsx so it fires last and has the final say. In practice the
 * carry-over flow either (a) opens a dialog (ask) — our pin runs in
 * the background and the user's subsequent dialog choice may override
 * us, which is correct (user agency wins), or (b) silently writes
 * `scheduled_date = today` or `null` (auto-today / auto-backlog) — in
 * both cases the pin's final write reconciles to today. The pin
 * itself does not snooze: it's silent and idempotent, so there's no
 * pestering to suppress.
 */
export function DeadlinePinChecker() {
  const { tasks, loading } = useTasks();
  const { mode: dialogMode, invalidateData } = useDialogState();
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const { dayStartTrigger, hydrating } = useTaskCascadeEnabled();
  const todayKey = useCurrentDayKey();
  const firedRef = useRef<string | null>(readFiredDayKey('deadlinePin'));

  useEffect(() => {
    if (loading || hydrating) return;
    if (!shouldFireToday(dayStartTrigger, firedRef.current, todayKey)) {
      return;
    }
    // Defer while an editor / dialog is open — silently changing
    // `scheduled_date` under an open task editor would let the user
    // save stale data and undo our pin a moment later.
    if (dialogMode.kind !== 'none') return;

    const targets = filterDeadlinePinTargets(tasks);
    firedRef.current = todayKey;
    writeFiredDayKey('deadlinePin', todayKey);
    if (targets.length === 0) return;

    void pinToToday(targets, announce, t, invalidateData);
  }, [
    loading,
    hydrating,
    tasks,
    dayStartTrigger,
    todayKey,
    dialogMode.kind,
    announce,
    t,
    invalidateData,
  ]);

  return null;
}

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

async function pinToToday(
  targets: Task[],
  announce: (message: string) => void,
  t: (key: string, values?: Record<string, unknown>) => string,
  invalidateData: () => void,
): Promise<void> {
  const today = todayIsoKey();
  const now = new Date().toISOString();
  try {
    await Promise.all(
      targets.map((task) =>
        invoke<Task>('update_task', {
          task: {
            ...task,
            scheduled_date: today,
            updated_at: now,
          },
        }),
      ),
    );
    announce(
      t('dialogs.deadlinePin.announce', { count: targets.length }),
    );
    invalidateData();
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn('deadline pin update_task failed', err);
  }
}
