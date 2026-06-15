import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/announcerContext';
import { isCommandError } from '../api/client';
import type { Task, TaskPriority } from '../api/types';
import { useDialogState } from './dialogStateContext';

/**
 * Set a task's priority.
 *
 * Unlike status, priority carries no cascade — it's a plain single-row
 * `update_task` (spread the task so every other field stays intact; only
 * `priority` changes). Shared by the chip/row context menu
 * ({@link useChipContextMenu}) and the editor's per-subtask menu
 * (TaskDialog) so the write, the screen-reader announcement and the error
 * handling stay identical wherever a quick priority change is offered.
 *
 * Re-picking the task's current priority is a no-op: no write fires and no
 * (misleading) "set to …" announcement is made.
 */
export function useTaskPriorityAction(): (
  task: Task,
  next: TaskPriority,
) => Promise<void> {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { invalidateData } = useDialogState();

  return useCallback(
    async (task: Task, next: TaskPriority): Promise<void> => {
      if (next === task.priority) return;
      try {
        await invoke<Task>('update_task', {
          task: { ...task, priority: next },
        });
        announce(
          t('chipMenu.prioritySet', {
            title: task.title,
            priority: t(`dialogs.task.priority.${next}`),
          }),
        );
        invalidateData();
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [t, announce, invalidateData],
  );
}
