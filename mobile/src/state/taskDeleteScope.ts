import type { Task, TaskList } from '@aperio/shared';
import { fromBackend, nextTaskOccurrence } from '@aperio/shared';

import { listAccounts } from '../api/accounts';
import { deleteTask, updateTask } from '../api/client';
import { showEventScopeDialog } from './eventScopeDialog';

// Recurrence-scoped delete for DEVICE reminders (iOS/Android EventKit /
// CalendarProvider), the task twin of confirmDeleteEvent. A recurring device
// reminder gets the iOS-Reminders-style choice — "only this occurrence" vs "this
// and all following" — while completed history is preserved either way. Every
// other task (non-recurring, or a local/external recurring task) keeps the plain
// confirm the caller already shows.
//
// - "this and all following" = the existing delete_task: removing the single live
//   recurring EKReminder ends the current + future turns; the separately-stored
//   completed reminders survive.
// - "only this occurrence" = NON-destructive: roll the reminder's due date forward
//   one step (via updateTask) so the series keeps going and just this turn is
//   skipped. When the series has no next step (past its UNTIL end), fall back to a
//   full delete.

type Tr = (key: string, vars?: Record<string, unknown>) => string;

// Device accounts don't change mid-session; cache the id set after the first
// SUCCESSFUL lookup. A load failure falls back to a plain delete for THIS
// call only — it must not be cached, or one early bridge hiccup would
// silently disable the scoped delete for the whole session (the option "just
// doesn't appear" and nothing ever says why).
let deviceAccountIds: Set<string> | null = null;

async function loadDeviceAccountIds(): Promise<Set<string>> {
  if (deviceAccountIds != null) return deviceAccountIds;
  try {
    const accounts = await listAccounts();
    deviceAccountIds = new Set(
      accounts
        .filter((a) => a.adapter_kind === 'device_calendar')
        .map((a) => a.id),
    );
    return deviceAccountIds;
  } catch {
    return new Set();
  }
}

function isDeviceRecurring(
  task: Task,
  taskLists: TaskList[],
  deviceIds: Set<string>,
): boolean {
  if (task.recurrence == null || task.scheduled_date == null) return false;
  const list = taskLists.find((l) => l.id === task.list_id);
  return list != null && deviceIds.has(list.account_id);
}

/**
 * Delete `task` with a recurrence-scope choice when it is a recurring DEVICE
 * reminder; otherwise call `onPlainDelete` (the caller's existing confirm). On a
 * successful scoped mutation calls `onSuccess(message)`, on failure
 * `onError(message)`.
 */
export function confirmDeleteTask(
  task: Task,
  taskLists: TaskList[],
  t: Tr,
  // `outcome` lets a screen-reader-first list restore focus correctly: a
  // 'removed' task focuses a surviving sibling / the empty state; a 'skipped'
  // one still exists (moved to the next date) and is re-focused itself.
  onSuccess: (message: string, outcome: 'removed' | 'skipped') => void,
  onError: (message: string) => void,
  onPlainDelete: () => void,
  // Fires the moment a scope option starts its mutation (never on cancel) —
  // the caller's chance to raise a busy flag so its surface can't race the
  // in-flight delete; onSuccess/onError are where it comes down again.
  onRun?: () => void,
): void {
  void (async () => {
    const deviceIds = await loadDeviceAccountIds();
    if (!isDeviceRecurring(task, taskLists, deviceIds)) {
      onPlainDelete();
      return;
    }

    const run = (
      fn: () => Promise<void>,
      message: string,
      outcome: 'removed' | 'skipped',
    ) => {
      onRun?.();
      void (async () => {
        try {
          await fn();
          onSuccess(message, outcome);
        } catch (err) {
          onError(err instanceof Error ? err.message : String(err));
        }
      })();
    };

    const deleteSeries = () =>
      run(
        () => deleteTask(task.id, task.list_id),
        t('dialogs.taskDeleteScope.deleted', { title: task.title }),
        'removed',
      );

    // Precompute the next occurrence for the "only this" branch. NOTE: only an
    // UNTIL end bounds this — a COUNT-bounded rule reaches the frontend as
    // "never-ending" (the count isn't tracked per device reminder), so "only
    // this" on the FINAL turn of a COUNT series rolls the due date one step past
    // the end instead of deleting. Harmless (a lingering reminder the user can
    // delete manually), and the provider still owns the count.
    const rule = fromBackend(task.recurrence);
    const next =
      task.scheduled_date != null
        ? nextTaskOccurrence(task.scheduled_date, rule)
        : null;

    showEventScopeDialog({
      title: t('dialogs.taskDeleteScope.title'),
      message: t('dialogs.taskDeleteScope.message', { title: task.title }),
      cancelLabel: t('mobile.cancel'),
      options: [
        {
          key: 'occurrence',
          label: t('dialogs.taskDeleteScope.occurrence'),
          run: () => {
            if (next == null) {
              // No later turn (series ended) → nothing to keep; remove it.
              deleteSeries();
            } else {
              run(
                () =>
                  updateTask({ ...task, scheduled_date: next }).then(() => {}),
                t('dialogs.taskDeleteScope.skipped', { title: task.title }),
                'skipped',
              );
            }
          },
        },
        {
          key: 'series',
          label: t('dialogs.taskDeleteScope.series'),
          destructive: true,
          run: () => deleteSeries(),
        },
      ],
    });
  })();
}
