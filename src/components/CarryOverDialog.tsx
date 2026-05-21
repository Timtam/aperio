import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import type { Task } from '../api/types';
import { todayIsoKey } from '../intl/taskDay';
import { useDialogState } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { Modal } from './Modal';

/**
 * Carry-over review dialog — companion to {@link MissedTasksDialog}.
 *
 * Triggered on app start (via {@link CarryOverChecker}) when any task
 * has a `scheduled_date` strictly before today and is still open or in
 * progress. The user can decide per row what to do with it. Closing
 * the dialog without resolving anything snoozes it for four hours,
 * same idiom as the missed-tasks variant.
 *
 * Distinct from `MissedTasksDialog` on purpose:
 *
 * - That one fires on **deadline** overruns (`deadline_date < today`)
 *   — the spec's "you said you'd be done by then, what now?" prompt.
 * - This one fires on **scheduled-day** overruns — "you said you'd
 *   work on it yesterday, where should it go now?". The chronic case
 *   the new auto-date feature was built for: a backlog task that
 *   bumped to today when the user marked it in_progress and then
 *   didn't make it across the finish line.
 *
 * Two separate dialogs because the conversation with the user is
 * subtly different — schedule slips offer "morgen übernehmen" and
 * "zurück in den Backlog", deadline slips don't. They share the
 * `MissedTasksChecker` neighbourhood in App.tsx but each is its own
 * mount-once gate so a snooze on one doesn't silence the other.
 */
export interface CarryOverDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export function CarryOverDialog({ isOpen, onClose }: CarryOverDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { tasks } = useTasks();
  const { invalidateData } = useDialogState();

  const slipped = useMemo(() => filterCarriedOver(tasks), [tasks]);
  const [busy, setBusy] = useState(false);
  // Mirror of `MissedTasksDialog.resolvedIds`: rows the user has just
  // handled disappear instantly even though the `update_task` round-
  // trip + cache refresh hasn't landed yet. Without this the same
  // row would stick around until invalidateData propagates and the
  // interaction would feel sluggish.
  const [resolvedIds, setResolvedIds] = useState<Set<string>>(new Set());
  const remaining = useMemo(
    () => slipped.filter((task) => !resolvedIds.has(task.id)),
    [slipped, resolvedIds],
  );

  // Once the list empties mid-session, snooze and close — no point
  // hanging around an empty dialog. The snooze prevents a re-open on
  // the next refetch in case the user reopens app within the window.
  useEffect(() => {
    if (!isOpen) return;
    if (remaining.length === 0 && resolvedIds.size > 0) {
      snoozeCarryOver(4);
      announce(t('dialogs.carryOver.allHandled'));
      onClose();
    }
  }, [isOpen, remaining.length, resolvedIds.size, announce, t, onClose]);

  /**
   * Single-row mutation runner. The four action buttons all feed
   * into this — they only differ in which field(s) they change.
   */
  const applyAction = useCallback(
    async (
      task: Task,
      patch: Partial<Task>,
      announcementKey: string,
    ): Promise<void> => {
      setBusy(true);
      try {
        const updated: Task = {
          ...task,
          ...patch,
          updated_at: new Date().toISOString(),
        };
        await invoke<Task>('update_task', { task: updated });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(t(announcementKey, { title: task.title }));
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

  const carryToToday = useCallback(
    (task: Task) =>
      applyAction(
        task,
        { scheduled_date: todayIsoKey() },
        'dialogs.carryOver.carriedToday',
      ),
    [applyAction],
  );

  const carryToTomorrow = useCallback(
    (task: Task) =>
      applyAction(
        task,
        { scheduled_date: tomorrowIsoKey() },
        'dialogs.carryOver.carriedTomorrow',
      ),
    [applyAction],
  );

  const sendToBacklog = useCallback(
    (task: Task) =>
      applyAction(
        task,
        { scheduled_date: null },
        'dialogs.carryOver.sentToBacklog',
      ),
    [applyAction],
  );

  const markCompleted = useCallback(
    (task: Task) =>
      applyAction(
        task,
        {
          status: 'completed',
          completed_at: new Date().toISOString(),
        },
        'dialogs.carryOver.completed',
      ),
    [applyAction],
  );

  const allToToday = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      const today = todayIsoKey();
      await Promise.all(
        remaining.map((task) =>
          invoke<Task>('update_task', {
            task: {
              ...task,
              scheduled_date: today,
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
        t('dialogs.carryOver.allCarriedToday', { count: remaining.length }),
      );
      invalidateData();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('batch update_task failed', err);
    } finally {
      setBusy(false);
    }
  }, [remaining, t, announce, invalidateData]);

  const allToBacklog = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      await Promise.all(
        remaining.map((task) =>
          invoke<Task>('update_task', {
            task: {
              ...task,
              scheduled_date: null,
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
        t('dialogs.carryOver.allSentToBacklog', { count: remaining.length }),
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
    snoozeCarryOver(4);
    announce(t('dialogs.carryOver.snoozed'));
    onClose();
  }, [t, announce, onClose]);

  // Defensive empty-state: caller opened us with nothing to show
  // (race between the checker and a refetch). Close quietly without
  // snoozing — there's no reason to suppress a real future trigger.
  if (remaining.length === 0 && resolvedIds.size === 0) {
    onClose();
    return null;
  }

  return (
    <Modal
      isOpen={isOpen}
      onClose={snoozeLater}
      title={t('dialogs.carryOver.title')}
      className="modal--form modal--carry-over"
    >
      <p className="form__hint">{t('dialogs.carryOver.hint')}</p>
      <ul
        className="missed-tasks__list"
        aria-label={t('dialogs.carryOver.listLabel')}
      >
        {remaining.map((task) => (
          <li key={task.id} className="missed-tasks__row">
            <div className="missed-tasks__title">
              <span className="missed-tasks__name">{task.title}</span>
              {task.scheduled_date && (
                <span className="missed-tasks__deadline">
                  {t('dialogs.carryOver.scheduledLabel', {
                    date: formatIsoDate(task.scheduled_date),
                  })}
                </span>
              )}
            </div>
            <div className="missed-tasks__actions">
              <button
                type="button"
                className="form__action form__action--primary"
                onClick={() => void carryToToday(task)}
                aria-disabled={busy || undefined}
              >
                {t('dialogs.carryOver.actions.today')}
              </button>
              <button
                type="button"
                className="form__action"
                onClick={() => void carryToTomorrow(task)}
                aria-disabled={busy || undefined}
              >
                {t('dialogs.carryOver.actions.tomorrow')}
              </button>
              <button
                type="button"
                className="form__action"
                onClick={() => void sendToBacklog(task)}
                aria-disabled={busy || undefined}
              >
                {t('dialogs.carryOver.actions.backlog')}
              </button>
              <button
                type="button"
                className="form__action"
                onClick={() => void markCompleted(task)}
                aria-disabled={busy || undefined}
              >
                {t('dialogs.carryOver.actions.done')}
              </button>
            </div>
          </li>
        ))}
      </ul>
      <div className="form__actions">
        <button
          type="button"
          className="form__action"
          onClick={() => void allToToday()}
          aria-disabled={busy || remaining.length === 0 || undefined}
        >
          {t('dialogs.carryOver.bulk.allToday')}
        </button>
        <button
          type="button"
          className="form__action"
          onClick={() => void allToBacklog()}
          aria-disabled={busy || remaining.length === 0 || undefined}
        >
          {t('dialogs.carryOver.bulk.allBacklog')}
        </button>
        <button type="button" className="form__action" onClick={snoozeLater}>
          {t('dialogs.carryOver.snooze')}
        </button>
      </div>
    </Modal>
  );
}

// ── Selection + snooze plumbing ────────────────────────────────────────

/**
 * Picks tasks with a `scheduled_date` strictly before today, still in
 * an actionable status (`open` or `in_progress`). Subtasks aren't
 * filtered out — if a child has slipped its own day, the user should
 * decide for it explicitly the same way they would for a top-level
 * row. The cascade-coupling preference doesn't enter into this
 * (scheduled_date is per-task; it never cascades).
 */
export function filterCarriedOver(tasks: Task[]): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.scheduled_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
    }
    return task.scheduled_date < today;
  });
}

const SNOOZE_KEY = 'aperio.carryOver.snoozeUntil';

/**
 * Suppress the carry-over dialog for `hours` hours. Stored separately
 * from the missed-tasks snooze key — snoozing one shouldn't quietly
 * silence the other.
 */
export function snoozeCarryOver(hours: number): void {
  try {
    const until = Date.now() + hours * 60 * 60 * 1000;
    localStorage.setItem(SNOOZE_KEY, String(until));
  } catch {
    // Storage unavailable; dialog will simply re-appear next start.
  }
}

export function isCarryOverSnoozed(): boolean {
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

/** Local `YYYY-MM-DD` for tomorrow — used by the "morgen" button. */
function tomorrowIsoKey(): string {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

function formatIsoDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString();
}
