import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import type { Task } from '../api/types';
import { useDialogState } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { Modal } from './Modal';

/**
 * Missed-task review dialog (DESIGN.md §9.5).
 *
 * Shown automatically on app start when any task has a deadline in
 * the past and is still open. The user can per-task tick "Erledigt"
 * or "Zurück in Backlog", or use the bottom actions "Alle erledigt"
 * (batch-complete every entry) and "Später erinnern" (suppress
 * re-showing for 4 hours).
 *
 * The list is read off `useTasks()` and refiltered on every render
 * so newly arriving tasks (sync mid-session) get picked up. Closing
 * the dialog explicitly via the X / Esc behaves like "Später
 * erinnern" — silent dismiss; we don't want to re-pester the user
 * five minutes later just because they hit Escape.
 */
export interface MissedTasksDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export function MissedTasksDialog({
  isOpen,
  onClose,
}: MissedTasksDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { tasks } = useTasks();
  const { invalidateData } = useDialogState();

  const overdue = useMemo(() => filterOverdue(tasks), [tasks]);
  const [busy, setBusy] = useState(false);
  // Tasks the user just resolved during this dialog session. We hide
  // them from the list immediately for instant feedback, even though
  // the actual update_task round-trip + cache refresh hasn't landed
  // yet. Without this the same row stays visible until invalidateData
  // propagates, which feels broken.
  const [resolvedIds, setResolvedIds] = useState<Set<string>>(new Set());
  const remaining = useMemo(
    () => overdue.filter((task) => !resolvedIds.has(task.id)),
    [overdue, resolvedIds],
  );

  // Close + snooze when the list empties out mid-session — the user
  // just cleared everything, no point keeping the modal up.
  useEffect(() => {
    if (!isOpen) return;
    if (remaining.length === 0 && resolvedIds.size > 0) {
      snoozeUntilNextHour(4);
      announce(t('dialogs.missedTasks.allHandled'));
      onClose();
    }
  }, [isOpen, remaining.length, resolvedIds.size, announce, t, onClose]);

  const markCompleted = useCallback(
    async (task: Task) => {
      setBusy(true);
      try {
        const updated: Task = {
          ...task,
          status: 'completed',
          completed_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        };
        await invoke<Task>('update_task', { task: updated });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.missedTasks.completed', { title: task.title }),
        );
        invalidateData();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      } finally {
        setBusy(false);
      }
    },
    [t, announce, invalidateData],
  );

  const backToBacklog = useCallback(
    async (task: Task) => {
      setBusy(true);
      try {
        const updated: Task = {
          ...task,
          scheduled_date: null,
          deadline_date: null,
          deadline_type: null,
          deadline_time: null,
          status: 'open',
          updated_at: new Date().toISOString(),
        };
        await invoke<Task>('update_task', { task: updated });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.missedTasks.backToBacklog', { title: task.title }),
        );
        invalidateData();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      } finally {
        setBusy(false);
      }
    },
    [t, announce, invalidateData],
  );

  const completeAll = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      await Promise.all(
        remaining.map((task) =>
          invoke<Task>('update_task', {
            task: {
              ...task,
              status: 'completed',
              completed_at: now,
              updated_at: now,
            },
          }),
        ),
      );
      setResolvedIds((s) => {
        const next = new Set(s);
        remaining.forEach((task) => next.add(task.id));
        return next;
      });
      announce(
        t('dialogs.missedTasks.allCompleted', { count: remaining.length }),
      );
      invalidateData();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('batch update_task failed', err);
    } finally {
      setBusy(false);
    }
  }, [remaining, t, announce, invalidateData]);

  const snoozeLater = useCallback(() => {
    snoozeUntilNextHour(4);
    announce(t('dialogs.missedTasks.snoozed'));
    onClose();
  }, [t, announce, onClose]);

  if (remaining.length === 0 && resolvedIds.size === 0) {
    // Defensive: caller decided to open but we have nothing to show.
    // Close cleanly without snoozing — there's nothing to snooze.
    onClose();
    return null;
  }

  return (
    <Modal
      isOpen={isOpen}
      onClose={snoozeLater}
      title={t('dialogs.missedTasks.title')}
      className="modal--form modal--missed-tasks"
    >
      <p className="form__hint">{t('dialogs.missedTasks.hint')}</p>
      <ul className="missed-tasks__list" aria-label={t('dialogs.missedTasks.listLabel')}>
        {remaining.map((task) => (
          <li key={task.id} className="missed-tasks__row">
            <div className="missed-tasks__title">
              <span className="missed-tasks__name">{task.title}</span>
              {task.deadline_date && (
                <span className="missed-tasks__deadline">
                  {t('dialogs.missedTasks.deadlineLabel', {
                    date: formatIsoDate(task.deadline_date),
                  })}
                </span>
              )}
            </div>
            <div className="missed-tasks__actions">
              <button
                type="button"
                className="form__action form__action--primary"
                onClick={() => void markCompleted(task)}
                aria-disabled={busy || undefined}
              >
                {t('dialogs.missedTasks.completed')}
              </button>
              <button
                type="button"
                className="form__action"
                onClick={() => void backToBacklog(task)}
                aria-disabled={busy || undefined}
              >
                {t('dialogs.missedTasks.backToBacklog')}
              </button>
            </div>
          </li>
        ))}
      </ul>
      <div className="form__actions">
        <button
          type="button"
          className="form__action"
          onClick={() => void completeAll()}
          aria-disabled={busy || remaining.length === 0 || undefined}
        >
          {t('dialogs.missedTasks.completeAll')}
        </button>
        <button type="button" className="form__action" onClick={snoozeLater}>
          {t('dialogs.missedTasks.snooze')}
        </button>
      </div>
    </Modal>
  );
}

// ── Selection + snooze plumbing ────────────────────────────────────────

/**
 * Today as the local `YYYY-MM-DD` key. Tasks store deadline_date as
 * `YYYY-MM-DD` (NaiveDate on the wire); string comparison is enough
 * to find overdue ones.
 */
function todayIsoLocal(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

/**
 * "Overdue" = has a deadline_date strictly before today AND is not
 * already completed/cancelled. `scheduled_date` alone doesn't count
 * — that's a planning hint, not a missed commitment.
 */
export function filterOverdue(tasks: Task[]): Task[] {
  const today = todayIsoLocal();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
    }
    return task.deadline_date < today;
  });
}

const SNOOZE_KEY = 'aperio.missedTasks.snoozeUntil';

/**
 * Suppress the dialog for `hours` hours by writing a deadline
 * timestamp to localStorage. `wasSnoozed` reads it back.
 */
export function snoozeUntilNextHour(hours: number): void {
  try {
    const until = Date.now() + hours * 60 * 60 * 1000;
    localStorage.setItem(SNOOZE_KEY, String(until));
  } catch {
    // localStorage unavailable (private mode, quota); the dialog
    // will just re-appear on next start, which is harmless.
  }
}

export function isCurrentlySnoozed(): boolean {
  try {
    const raw = localStorage.getItem(SNOOZE_KEY);
    if (!raw) return false;
    const until = Number.parseInt(raw, 10);
    if (Number.isNaN(until)) return false;
    return Date.now() < until;
  } catch {
    return false;
  }
}

function formatIsoDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString();
}
