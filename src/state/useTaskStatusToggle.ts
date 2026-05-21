import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import { useDialogState } from './DialogState';
import type { Task, TaskStatus } from '../api/types';

/**
 * Shared task-status mutations. Every task surface (TaskView, plus
 * the calendar chips in WeekView and DayView) needs:
 *
 *   - `toggle(task)` — Space-key contract from §9.4: flip between
 *     `open` and `completed`. Mirrors the visible ☐/☑ marker.
 *   - `set(task, status)` — explicit assignment, used by the chip
 *     context menu's "Status > {Offen, In Arbeit, Erledigt,
 *     Abgebrochen}" submenu.
 *
 * Both routes share one mutation path: `update_task` with a fresh
 * `completed_at` (set on Completed, cleared on every other status),
 * a `dataVersion` bump so `useTasks` refetches, and an SR live-region
 * announce. Inlining drifted across views before this hook existed
 * — keeping the contract in one place ensures the keyboard, the
 * checkbox marker, and the menu all behave identically.
 *
 * Returns an object instead of a plain function so the hook can be
 * destructured as `const { toggle, set } = useTaskStatusActions()`.
 * The compatibility default export `useTaskStatusToggle` returns
 * just the toggle callable for older call sites that haven't been
 * updated, and is shadowed by the new hook in the same module so a
 * single import gives the caller whichever shape it wants.
 */

export interface TaskStatusActions {
  toggle: (task: Task) => Promise<void>;
  set: (task: Task, status: TaskStatus) => Promise<void>;
}

export function useTaskStatusActions(): TaskStatusActions {
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const { invalidateData } = useDialogState();

  // Shared write path. `set` does the real work; `toggle` is a thin
  // wrapper that picks the next status. Keeping them on the same
  // useCallback chain means a single allocation per render.
  const set = useCallback(
    async (task: Task, nextStatus: TaskStatus): Promise<void> => {
      if (task.status === nextStatus) return;
      const updated: Task = {
        ...task,
        status: nextStatus,
        completed_at:
          nextStatus === 'completed' ? new Date().toISOString() : null,
      };
      try {
        await invoke<Task>('update_task', { task: updated });
        invalidateData();
        // Pick the announce flavour from the target state — the same
        // strings the toggle path used historically. New keys cover
        // the "in_progress" and "cancelled" cases the toggle path
        // never produced.
        announce(announceFor(t, nextStatus, task.title));
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      }
    },
    [announce, t, invalidateData],
  );

  const toggle = useCallback(
    async (task: Task): Promise<void> => {
      const nextStatus: TaskStatus =
        task.status === 'completed' ? 'open' : 'completed';
      await set(task, nextStatus);
    },
    [set],
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
