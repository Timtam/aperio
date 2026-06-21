import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/announcerContext';
import type { Task } from '../api/types';
import { todayIsoKey } from '../intl/taskDay';
import { priorityI18nKey, priorityMarker } from '../intl/taskStatus';
import { useCurrentUserByList } from '../state/currentUser';
import { useDialogState } from '../state/dialogStateContext';
import { useTaskCascadeEnabled } from '../state/taskCascadeContext';
import { useTaskStatusActions } from '../state/useTaskStatusToggle';
import { useTasks } from '../state/useTasks';
import {
  actionableDescendants,
  filterCarriedOver,
  filterOverdue,
  snoozeDayStartReview,
} from './dayStartReview';
import { Modal } from './Modal';

/**
 * Day-start review dialog (DESIGN.md § 9.5).
 *
 * The single home for the "what changed overnight" prompt. Two
 * sections:
 *
 *   1. **Verpasste Deadlines** — tasks whose `deadline_date` lapsed
 *      before today. Per-row actions: Erledigt, Zurück in Backlog.
 *      The deadline was the binding commitment so "today / tomorrow"
 *      buttons would side-step the actual decision (was it done? was
 *      it dropped?) and silently rewrite a missed promise into a
 *      planning hint.
 *   2. **Liegengebliebene Aufgaben** — tasks whose `scheduled_date`
 *      lapsed before today but no deadline crossed. Per-row actions:
 *      Heute, Morgen, Backlog, Erledigt. Bulk: "Alle auf heute" /
 *      "Alle in Backlog".
 *
 * A task that satisfies both filters (deadline AND scheduled_date
 * before today) appears only in the deadline section — the deadline
 * is the bigger lever, and Back-to-Backlog there covers the same
 * outcome the carry-over Backlog button would. {@link filterCarriedOver}
 * does the dedup.
 *
 * Cascade-coupling (Settings → Tasks): when on, subtasks whose
 * ancestor is also slipped are hidden, and date-change actions on a
 * parent row drag the actionable children along. Erledigt routes
 * through the shared status action so it picks up the same cascade
 * rule used everywhere else.
 *
 * Snooze: closing the dialog (X / Esc / "Später erinnern" button)
 * snoozes the whole gate for four hours. The two-dialog era kept
 * separate snooze flags; with a single combined surface that
 * distinction doesn't map to anything the user can choose.
 *
 * Auto-close: once the user has resolved every visible row, the
 * dialog closes itself (and snoozes) — no point keeping an empty
 * modal up. The empty-on-open case is handled defensively too.
 */
export interface DayStartReviewDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export function DayStartReviewDialog({
  isOpen,
  onClose,
}: DayStartReviewDialogProps) {
  const { t, i18n } = useTranslation();
  const announce = useAnnouncer();
  const { tasks } = useTasks();
  const { invalidateData } = useDialogState();
  // Per-list cascade resolution: each row obeys its OWN list's
  // status-coupling preference. The filter below treats a slipped
  // subtask as hidden iff its own list has cascade on AND it has a
  // slipped ancestor in that same list; the per-row action
  // handlers walk descendants the same way.
  const { effectiveForList } = useTaskCascadeEnabled();
  const cascadeEnabledFor = useCallback(
    (listId: string) => effectiveForList(listId).cascade,
    [effectiveForList],
  );
  const { set: setTaskStatus } = useTaskStatusActions();

  // Only my own / unassigned tasks are offered; a task owned by a concrete other
  // user is someone else's to handle (DESIGN §9.7). `meFor` resolves the
  // connected user per list from the session cache (null = no identity ⇒ keep).
  const currentUserByList = useCurrentUserByList(tasks);
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );

  // The dialog auto-opens (no click trigger) and its task rows only render once
  // its own useTasks re-fetch settles — so the Modal's "focus the first
  // focusable" would find nothing yet and strand the screen reader. Focus this
  // always-present intro instead: it lands inside the role="application" body
  // (focus mode) and reads the dialog title + instructions first.
  const introRef = useRef<HTMLParagraphElement>(null);

  const overdue = useMemo(() => filterOverdue(tasks, meFor), [tasks, meFor]);
  const slipped = useMemo(
    () => filterCarriedOver(tasks, { cascadeEnabledFor, meFor }),
    [tasks, cascadeEnabledFor, meFor],
  );

  const [busy, setBusy] = useState(false);
  // Rows the user has just handled disappear instantly even though
  // the `update_task` round-trip + cache refresh hasn't landed yet.
  // Without this, rows stick around until invalidateData propagates
  // and every click feels laggy.
  const [resolvedIds, setResolvedIds] = useState<Set<string>>(new Set());

  const remainingOverdue = useMemo(
    () => overdue.filter((task) => !resolvedIds.has(task.id)),
    [overdue, resolvedIds],
  );
  const remainingSlipped = useMemo(
    () => slipped.filter((task) => !resolvedIds.has(task.id)),
    [slipped, resolvedIds],
  );
  const totalRemaining = remainingOverdue.length + remainingSlipped.length;

  // Close + snooze when the list empties out mid-session — the user
  // just cleared everything, no point keeping the modal up.
  useEffect(() => {
    if (!isOpen) return;
    if (totalRemaining === 0 && resolvedIds.size > 0) {
      snoozeDayStartReview(4);
      announce(t('dialogs.dayStartReview.allHandled'));
      onClose();
    }
  }, [isOpen, totalRemaining, resolvedIds.size, announce, t, onClose]);

  // ── Deadline-section actions ────────────────────────────────────────

  const markCompleted = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      try {
        // Delegate to the shared status action — it runs the same
        // cascade rules as the rest of the app, applies the
        // completed_at + auto-date dance, and fires its own
        // announcement. We just need to hide the dialog row.
        await setTaskStatus(task, 'completed');
        setResolvedIds((s) => new Set(s).add(task.id));
      } finally {
        setBusy(false);
      }
    },
    [setTaskStatus],
  );

  const backToBacklog = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      try {
        const updated: Task = {
          ...task,
          scheduled_date: null,
          scheduled_time: null,
          deadline_date: null,
          deadline_time: null,
          status: 'open',
          updated_at: new Date().toISOString(),
        };
        await invoke<Task>('update_task', { task: updated });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.dayStartReview.deadlines.announceBackToBacklog', {
            title: task.title,
          }),
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

  const completeAllOverdue = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      await Promise.all(
        remainingOverdue.map((task) =>
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
        remainingOverdue.forEach((task) => next.add(task.id));
        return next;
      });
      announce(
        t('dialogs.dayStartReview.deadlines.announceAllCompleted', {
          count: remainingOverdue.length,
        }),
      );
      invalidateData();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('batch update_task failed', err);
    } finally {
      setBusy(false);
    }
  }, [remainingOverdue, t, announce, invalidateData]);

  // ── Carry-over section actions ──────────────────────────────────────

  /**
   * Apply a date-change patch to a root row and, when coupling is on,
   * to its actionable descendants too. The three date-flavour
   * buttons (Heute / Morgen / Backlog) all funnel through this; the
   * Erledigt button goes a different route via the shared status
   * action so the status cascade rules stay centralised.
   */
  const applyDateAction = useCallback(
    async (
      root: Task,
      newDate: string | null,
      announcementKey: string,
    ): Promise<void> => {
      setBusy(true);
      try {
        // Per-list cascade: descendants follow the action iff THIS
        // row's list has cascade on. A user with cascade off for
        // "Hobby" can move the parent without dragging the kids
        // along, even if "Work" cascades.
        const cascade = effectiveForList(root.list_id).cascade;
        const followers = cascade
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
    [tasks, effectiveForList, t, announce, invalidateData],
  );

  const carryToToday = useCallback(
    (task: Task) =>
      applyDateAction(
        task,
        todayIsoKey(),
        'dialogs.dayStartReview.carryOver.announceToday',
      ),
    [applyDateAction],
  );
  const carryToTomorrow = useCallback(
    (task: Task) =>
      applyDateAction(
        task,
        tomorrowIsoKey(),
        'dialogs.dayStartReview.carryOver.announceTomorrow',
      ),
    [applyDateAction],
  );
  const sendToBacklog = useCallback(
    (task: Task) =>
      applyDateAction(
        task,
        null,
        'dialogs.dayStartReview.carryOver.announceBacklog',
      ),
    [applyDateAction],
  );

  /**
   * Compute the full set of writes for a bulk action — every visible
   * carry-over row plus, when coupling is on, its actionable
   * descendants. The Set guards against the same task ID landing
   * twice via overlapping branches; with the dialog's "no slipped
   * ancestor" filter that shouldn't actually happen, but it's a
   * cheap safety net.
   */
  const collectBulkCarryTargets = useCallback((): Task[] => {
    const collected = new Map<string, Task>();
    for (const row of remainingSlipped) {
      collected.set(row.id, row);
      // Per-list cascade — same logic as the per-row action
      // above, just applied to every visible carry-over row.
      if (!effectiveForList(row.list_id).cascade) continue;
      for (const desc of actionableDescendants(row.id, tasks)) {
        collected.set(desc.id, desc);
      }
    }
    return [...collected.values()];
  }, [remainingSlipped, effectiveForList, tasks]);

  const allCarryToToday = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      const today = todayIsoKey();
      const targets = collectBulkCarryTargets();
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
        t('dialogs.dayStartReview.carryOver.announceAllToday', {
          count: remainingSlipped.length,
        }),
      );
      invalidateData();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('batch update_task failed', err);
    } finally {
      setBusy(false);
    }
  }, [remainingSlipped, collectBulkCarryTargets, t, announce, invalidateData]);

  const allCarryToBacklog = useCallback(async () => {
    setBusy(true);
    try {
      const now = new Date().toISOString();
      const targets = collectBulkCarryTargets();
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
        t('dialogs.dayStartReview.carryOver.announceAllBacklog', {
          count: remainingSlipped.length,
        }),
      );
      invalidateData();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('batch update_task failed', err);
    } finally {
      setBusy(false);
    }
  }, [remainingSlipped, collectBulkCarryTargets, t, announce, invalidateData]);

  // ── Snooze + empty state ────────────────────────────────────────────

  const snoozeLater = useCallback(() => {
    snoozeDayStartReview(4);
    announce(t('dialogs.dayStartReview.snoozed'));
    onClose();
  }, [t, announce, onClose]);

  // Defensive empty-state: caller opened us with nothing to show
  // (race between the checker and a refetch). Close quietly without
  // snoozing — there's no reason to suppress a real future trigger.
  if (totalRemaining === 0 && resolvedIds.size === 0) {
    onClose();
    return null;
  }

  return (
    <Modal
      isOpen={isOpen}
      onClose={snoozeLater}
      title={t('dialogs.dayStartReview.title')}
      className="modal--form modal--day-start-review"
      initialFocusRef={introRef}
    >
      <p ref={introRef} tabIndex={-1} className="form__hint">
        {t('dialogs.dayStartReview.hint')}
      </p>

      {remainingOverdue.length > 0 && (
        <section className="day-start-review__section">
          <h3 className="day-start-review__heading">
            {t('dialogs.dayStartReview.deadlines.heading', {
              count: remainingOverdue.length,
            })}
          </h3>
          <ul
            className="missed-tasks__list"
            aria-label={t('dialogs.dayStartReview.deadlines.listLabel')}
          >
            {remainingOverdue.map((task) => (
              <li key={task.id} className="missed-tasks__row">
                <div className="missed-tasks__title">
                  <span className="missed-tasks__name">{task.title}</span>
                  {priorityMarker(task.priority) && (
                    <span
                      className="missed-tasks__priority"
                      aria-label={t(priorityI18nKey(task.priority) ?? '')}
                    >
                      {priorityMarker(task.priority)}
                    </span>
                  )}
                  {task.deadline_date && (
                    <span className="missed-tasks__deadline">
                      {t('dialogs.dayStartReview.deadlines.dateLabel', {
                        date: formatIsoDate(task.deadline_date, i18n.language),
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
                    {t('dialogs.dayStartReview.deadlines.actions.done')}
                  </button>
                  <button
                    type="button"
                    className="form__action"
                    onClick={() => void backToBacklog(task)}
                    aria-disabled={busy || undefined}
                  >
                    {t('dialogs.dayStartReview.deadlines.actions.backlog')}
                  </button>
                </div>
              </li>
            ))}
          </ul>
          <div className="day-start-review__section-actions">
            <button
              type="button"
              className="form__action"
              onClick={() => void completeAllOverdue()}
              aria-disabled={
                busy || remainingOverdue.length === 0 || undefined
              }
            >
              {t('dialogs.dayStartReview.deadlines.bulk.allDone')}
            </button>
          </div>
        </section>
      )}

      {remainingSlipped.length > 0 && (
        <section className="day-start-review__section">
          <h3 className="day-start-review__heading">
            {t('dialogs.dayStartReview.carryOver.heading', {
              count: remainingSlipped.length,
            })}
          </h3>
          <ul
            className="missed-tasks__list"
            aria-label={t('dialogs.dayStartReview.carryOver.listLabel')}
          >
            {remainingSlipped.map((task) => (
              <li key={task.id} className="missed-tasks__row">
                <div className="missed-tasks__title">
                  <span className="missed-tasks__name">{task.title}</span>
                  {priorityMarker(task.priority) && (
                    <span
                      className="missed-tasks__priority"
                      aria-label={t(priorityI18nKey(task.priority) ?? '')}
                    >
                      {priorityMarker(task.priority)}
                    </span>
                  )}
                  {task.scheduled_date && (
                    <span className="missed-tasks__deadline">
                      {t('dialogs.dayStartReview.carryOver.dateLabel', {
                        date: formatIsoDate(task.scheduled_date, i18n.language),
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
                    {t('dialogs.dayStartReview.carryOver.actions.today')}
                  </button>
                  <button
                    type="button"
                    className="form__action"
                    onClick={() => void carryToTomorrow(task)}
                    aria-disabled={busy || undefined}
                  >
                    {t('dialogs.dayStartReview.carryOver.actions.tomorrow')}
                  </button>
                  <button
                    type="button"
                    className="form__action"
                    onClick={() => void sendToBacklog(task)}
                    aria-disabled={busy || undefined}
                  >
                    {t('dialogs.dayStartReview.carryOver.actions.backlog')}
                  </button>
                  <button
                    type="button"
                    className="form__action"
                    onClick={() => void markCompleted(task)}
                    aria-disabled={busy || undefined}
                  >
                    {t('dialogs.dayStartReview.carryOver.actions.done')}
                  </button>
                </div>
              </li>
            ))}
          </ul>
          <div className="day-start-review__section-actions">
            <button
              type="button"
              className="form__action"
              onClick={() => void allCarryToToday()}
              aria-disabled={
                busy || remainingSlipped.length === 0 || undefined
              }
            >
              {t('dialogs.dayStartReview.carryOver.bulk.allToday')}
            </button>
            <button
              type="button"
              className="form__action"
              onClick={() => void allCarryToBacklog()}
              aria-disabled={
                busy || remainingSlipped.length === 0 || undefined
              }
            >
              {t('dialogs.dayStartReview.carryOver.bulk.allBacklog')}
            </button>
          </div>
        </section>
      )}

      <div className="form__actions">
        <button type="button" className="form__action" onClick={snoozeLater}>
          {t('dialogs.dayStartReview.snooze')}
        </button>
      </div>
    </Modal>
  );
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

function formatIsoDate(iso: string, locale: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(locale, { dateStyle: 'long' });
}
