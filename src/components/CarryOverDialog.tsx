import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import type { Task } from '../api/types';
import { todayIsoKey } from '../intl/taskDay';
import { useDialogState } from '../state/DialogState';
import { useTaskCascadeEnabled } from '../state/TaskCascadeProvider';
import { useTaskStatusActions } from '../state/useTaskStatusToggle';
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
  // Status-coupling preference (Settings → Tasks). When on, subtasks
  // are hidden from the dialog if their parent is also slipped, and
  // date-change actions on a parent row drag the actionable children
  // along. Erledigt routes through the shared status action so it
  // picks up the same cascade rule used everywhere else.
  const { enabled: cascadeEnabled } = useTaskCascadeEnabled();
  const { set: setTaskStatus } = useTaskStatusActions();

  const slipped = useMemo(
    () => filterCarriedOver(tasks, { cascadeEnabled }),
    [tasks, cascadeEnabled],
  );
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
   * Apply a date-change patch to a root row and, when coupling is on,
   * to its actionable descendants too. The three date-flavour
   * buttons (Heute / Morgen / Backlog) all funnel through this; the
   * Erledigt button goes a different route via the shared status
   * action so the status cascade rules stay centralised.
   *
   * The announce string carries a cascade-count suffix on coupling-on
   * runs with at least one descendant, so SR users know they didn't
   * just touch a single row — matching the convention already used
   * by useTaskStatusToggle.
   */
  const applyDateAction = useCallback(
    async (
      root: Task,
      newDate: string | null,
      announcementKey: string,
    ): Promise<void> => {
      setBusy(true);
      try {
        const followers = cascadeEnabled
          ? actionableDescendants(root.id, tasks)
          : [];
        const targets: Task[] = [root, ...followers];
        const now = new Date().toISOString();
        // Sequential so a first-row failure surfaces cleanly without
        // leaving a half-applied family. Typical depth is one or two
        // so this stays well under the SQLite write budget.
        for (const target of targets) {
          await invoke<Task>('update_task', {
            task: {
              ...target,
              scheduled_date: newDate,
              updated_at: now,
            },
          });
        }
        setResolvedIds((s) => {
          const next = new Set(s);
          targets.forEach((task) => next.add(task.id));
          return next;
        });
        const base = t(announcementKey, { title: root.title });
        // The cascadeSuffix translation key lives under `views.tasks`
        // because useTaskStatusToggle owns the canonical announcement
        // ("Aufgabe X erledigt. N weitere Aufgaben wurden mit
        // aktualisiert."). We piggy-back on it here so a coupled
        // carry-over move sounds identical to a coupled status flip.
        announce(
          followers.length > 0
            ? `${base} ${t('views.tasks.cascadeSuffix', {
                count: followers.length,
              })}`
            : base,
        );
        invalidateData();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      } finally {
        setBusy(false);
      }
    },
    [tasks, cascadeEnabled, t, announce, invalidateData],
  );

  const carryToToday = useCallback(
    (task: Task) =>
      applyDateAction(
        task,
        todayIsoKey(),
        'dialogs.carryOver.carriedToday',
      ),
    [applyDateAction],
  );

  const carryToTomorrow = useCallback(
    (task: Task) =>
      applyDateAction(
        task,
        tomorrowIsoKey(),
        'dialogs.carryOver.carriedTomorrow',
      ),
    [applyDateAction],
  );

  const sendToBacklog = useCallback(
    (task: Task) =>
      applyDateAction(
        task,
        null,
        'dialogs.carryOver.sentToBacklog',
      ),
    [applyDateAction],
  );

  const markCompleted = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      try {
        // Delegate to the shared status action — it runs
        // planStatusCascade with the current coupling preference,
        // applies every resulting write (including the
        // completed_at + auto-date dance), and announces via the
        // global Announcer. We only need to hide the dialog row
        // here; cascaded descendants weren't shown in the first
        // place when coupling is on, and aren't expected to be on
        // when coupling is off.
        await setTaskStatus(task, 'completed');
        setResolvedIds((s) => new Set(s).add(task.id));
      } finally {
        setBusy(false);
      }
    },
    [setTaskStatus],
  );

  /**
   * Compute the full set of writes for a bulk action — every visible
   * row plus, when coupling is on, its actionable descendants. The
   * Set guards against the same task ID landing twice via overlapping
   * branches; with the dialog's "no slipped ancestor" filter that
   * shouldn't actually happen, but it's a cheap safety net.
   */
  const collectBulkTargets = useCallback((): Task[] => {
    const collected = new Map<string, Task>();
    for (const row of remaining) {
      collected.set(row.id, row);
      if (!cascadeEnabled) continue;
      for (const desc of actionableDescendants(row.id, tasks)) {
        collected.set(desc.id, desc);
      }
    }
    return [...collected.values()];
  }, [remaining, cascadeEnabled, tasks]);

  const allToToday = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      const today = todayIsoKey();
      const targets = collectBulkTargets();
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
      setResolvedIds((s) => {
        const next = new Set(s);
        targets.forEach((task) => next.add(task.id));
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
  }, [remaining, collectBulkTargets, t, announce, invalidateData]);

  const allToBacklog = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      const targets = collectBulkTargets();
      await Promise.all(
        targets.map((task) =>
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
        targets.forEach((task) => next.add(task.id));
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
  }, [remaining, collectBulkTargets, t, announce, invalidateData]);

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
 * an actionable status (`open` or `in_progress`).
 *
 * When `cascadeEnabled` is true (the carry-over honours the Settings →
 * Tasks status-coupling preference) a slipped task is hidden if any
 * ancestor is also slipped — the user only decides at the root of
 * each slipped subtree, and the dialog's action handlers propagate
 * the chosen verdict (Heute / Morgen / Backlog / Erledigt) to the
 * actionable descendants. An "orphaned" slipped subtask whose parent
 * itself isn't slipped still surfaces as its own row, because there
 * is no slipped ancestor to attach it to.
 *
 * When `cascadeEnabled` is false (or omitted, for backward
 * compatibility with the checker / tests) every slipped task appears
 * as its own row regardless of hierarchy.
 */
export function filterCarriedOver(
  tasks: Task[],
  options?: { cascadeEnabled?: boolean },
): Task[] {
  const today = todayIsoKey();
  const slipped = tasks.filter((task) => {
    if (!task.scheduled_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
    }
    return task.scheduled_date < today;
  });

  if (!options?.cascadeEnabled) return slipped;

  const slippedIds = new Set(slipped.map((t) => t.id));
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const hasSlippedAncestor = (task: Task): boolean => {
    let parentId: string | null = task.parent_id;
    while (parentId) {
      if (slippedIds.has(parentId)) return true;
      parentId = byId.get(parentId)?.parent_id ?? null;
    }
    return false;
  };
  return slipped.filter((task) => !hasSlippedAncestor(task));
}

/**
 * Walk all descendants of `rootId` and collect the ones still in an
 * actionable status (`open` or `in_progress`). Used by the dialog's
 * action handlers when status-coupling is on: a Heute / Morgen /
 * Backlog click on a parent row needs to drag its open children
 * along, but should leave already-completed or cancelled descendants
 * alone — those have a settled scheduled_date that records when the
 * work actually happened (or was dropped), and overwriting it would
 * silently rewrite history.
 */
export function actionableDescendants(
  rootId: string,
  tasks: Task[],
): Task[] {
  const out: Task[] = [];
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    for (const t of tasks) {
      if (t.parent_id !== id) continue;
      stack.push(t.id);
      if (t.status === 'open' || t.status === 'in_progress') {
        out.push(t);
      }
    }
  }
  return out;
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
