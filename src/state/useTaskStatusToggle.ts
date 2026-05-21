import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import { useDialogState } from './DialogState';
import type { Task, TaskStatus } from '../api/types';

/**
 * Flip the focused task between "open" and "completed" with the same
 * mutation path TaskView has used since Phase 9.1, exposed as a hook
 * so calendar surfaces (WeekView, DayView) can share it without
 * re-implementing the optimistic-announce + invalidate-data dance.
 *
 * Why the indirection: every task surface needs the same three things
 * after a status flip — the row's `completed_at` has to be set or
 * cleared, the data cache has to refetch so the new sort bucket /
 * status badge lands everywhere, and the SR has to hear what the
 * keypress just did. Inlining that in each view drifted (TaskView
 * announces "erledigt / wieder offen", WeekView used to do nothing).
 * The hook locks the contract.
 *
 * Returns an async function so the caller can `void` it from a
 * keypress handler without React complaining about returning a
 * Promise from an event listener.
 */
export function useTaskStatusToggle(): (task: Task) => Promise<void> {
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const { invalidateData } = useDialogState();

  return useCallback(
    async (task: Task) => {
      const nextStatus: TaskStatus =
        task.status === 'completed' ? 'open' : 'completed';
      const updated: Task = {
        ...task,
        status: nextStatus,
        completed_at:
          nextStatus === 'completed' ? new Date().toISOString() : null,
      };
      try {
        await invoke<Task>('update_task', { task: updated });
        // The toggle never opens a global dialog (it's an in-place
        // status change), so useTasks would otherwise never see the
        // mutation. Bump explicitly to refetch and reflect the new
        // sort bucket.
        invalidateData();
        announce(
          nextStatus === 'completed'
            ? t('views.tasks.completedAnnounce', { title: task.title })
            : t('views.tasks.reopenedAnnounce', { title: task.title }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      }
    },
    [announce, t, invalidateData],
  );
}
