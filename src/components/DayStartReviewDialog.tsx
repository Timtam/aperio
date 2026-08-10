import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import {
  buildReminderGroups,
  daysUntilDeadline,
  occurrenceMoveTarget,
} from '@aperio/shared';

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
  deadlineMovedToToday,
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
  const { tasks, taskListById, loading: tasksLoading } = useTasks();
  const { invalidateData, openTaskDialog } = useDialogState();
  // Per-list cascade resolution: each row obeys its OWN list's
  // status-coupling preference. The filter below treats a slipped
  // subtask as hidden iff its own list has cascade on AND it has a
  // slipped ancestor in that same list; the per-row action
  // handlers walk descendants the same way.
  const {
    effectiveForList,
    remindUntimedToday,
    remindDeadlineArrived,
    remindDeadlineCountdown,
    deadlineCountdownDays,
  } = useTaskCascadeEnabled();
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

  // Read-only reminder groups, gated by the same Settings toggles the
  // checker uses. Informational: the rows open the task editor but
  // never mutate state from the dialog, so they're outside the
  // resolved/snooze bookkeeping below. The SHARED `buildReminderGroups`
  // de-duplicates by task id (a deadline-pinned task surfaces in exactly
  // one group), so the rows here match the checker's spoken count and OS
  // notification exactly — no task appears in two sections.
  const groups = useMemo(
    () =>
      buildReminderGroups(
        tasks,
        {
          remindUntimedToday,
          remindDeadlineArrived,
          remindDeadlineCountdown,
          deadlineCountdownDays,
        },
        meFor,
      ),
    [
      tasks,
      remindUntimedToday,
      remindDeadlineArrived,
      remindDeadlineCountdown,
      deadlineCountdownDays,
      meFor,
    ],
  );
  const hasReminders =
    groups.untimed.length + groups.dueToday.length + groups.countdown.length >
    0;

  // Render-ready reminder groups: the summary line (count-aware) + a
  // per-row "why" suffix for each task's button label, paired with the
  // matching task set. Empty groups are filtered out at render time. The
  // countdown "why" is PER TASK — the set spans the whole 1..window range,
  // so each task announces its OWN remaining days — hence `why` is a function.
  const reminderGroups = useMemo(
    () => [
      {
        key: 'untimed',
        tasks: groups.untimed,
        summary: t('dialogs.dayStartReview.reminders.untimedToday', {
          count: groups.untimed.length,
        }),
        why: () => t('dialogs.dayStartReview.reminders.whyUntimed'),
      },
      {
        key: 'dueToday',
        tasks: groups.dueToday,
        summary: t('dialogs.dayStartReview.reminders.deadlineArrived', {
          count: groups.dueToday.length,
        }),
        why: () => t('dialogs.dayStartReview.reminders.whyDeadlineToday'),
      },
      {
        key: 'countdown',
        tasks: groups.countdown,
        summary: t('dialogs.dayStartReview.reminders.countdown', {
          count: groups.countdown.length,
        }),
        why: (task: Task) =>
          t('dialogs.dayStartReview.reminders.whyCountdown', {
            lead: t('dialogs.dayStartReview.reminders.inDays', {
              count: daysUntilDeadline(task) ?? deadlineCountdownDays,
            }),
          }),
      },
    ],
    [groups, deadlineCountdownDays, t],
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

  // Reminders-only review: nothing left to decide, just today's read-only
  // nudges. "Später erinnern" is meaningless here (no work to defer, and the
  // day is already marked reviewed before the dialog opens — the gate won't
  // re-fire today either way), so the footer offers a plain acknowledge, Escape
  // and the × close without burning a snooze, and the intro copy swaps.
  const remindersOnly = totalRemaining === 0 && hasReminders;

  // Close when the actionable list empties out mid-session — the user just
  // cleared everything, no point keeping the modal up.
  //
  // …UNLESS reminders are still on screen. The day's fire marker is written
  // BEFORE the dialog opens, so a close here is FINAL for today: tearing the
  // dialog down the moment the last deadline is ticked off would silently eat
  // reminders the user never read. Fall through to the reminders-only state
  // instead (the announce + focus repark below carry the transition).
  useEffect(() => {
    if (!isOpen) return;
    if (totalRemaining > 0 || resolvedIds.size === 0) return;
    if (hasReminders) return;
    snoozeDayStartReview(4);
    announce(t('dialogs.dayStartReview.allHandled'));
    onClose();
  }, [
    isOpen,
    totalRemaining,
    resolvedIds.size,
    hasReminders,
    announce,
    t,
    onClose,
  ]);

  // Speak the switch INTO the reminders-only state. The intro's text (and its
  // aria-label) and the footer button's identity both change here, and a
  // focused element is not re-read when its label changes — so a silent swap
  // would leave the screen reader describing a dialog that no longer exists.
  // Two ways in: the user cleared the last actionable row (announce what
  // happened; the repark below then lands on the intro, which reads the new
  // hint), or the rows were never really there (a stale `tasks` read on the
  // mount render) — then focus is already parked on the intro and only the
  // text changed, so the hint itself is what needs speaking.
  const wasRemindersOnly = useRef(false);
  useEffect(() => {
    if (!isOpen) {
      wasRemindersOnly.current = false;
      return;
    }
    if (remindersOnly && !wasRemindersOnly.current) {
      announce(
        resolvedIds.size > 0
          ? t('dialogs.dayStartReview.allHandled')
          : t('dialogs.dayStartReview.hintRemindersOnly'),
      );
    }
    wasRemindersOnly.current = remindersOnly;
  }, [isOpen, remindersOnly, resolvedIds.size, announce, t]);

  // Each per-row action unmounts the very button that was pressed, dropping
  // focus to <body> — outside #app-root's role="application", so NVDA leaves
  // application mode and Escape/Tab go dead while the dialog is still up. When a
  // row resolves and others remain, repark focus onto the next actionable
  // control so the user keeps working through the list without the reader
  // falling out. A useLayoutEffect runs in the same commit (before Modal's
  // last-resort recovery frame), so this informed repark wins. Gated on
  // resolvedIds.size so it never fires on first open (Modal's initialFocusRef
  // owns that). Only acts when focus actually fell outside the dialog body — a
  // still-mounted bulk button keeping focus is left alone. When the last
  // actionable row goes and reminders keep the dialog up, there is no next
  // action button to land on: fall back to the intro, whose aria-label now
  // carries the reminders-only hint, so the screen reader describes the state
  // the dialog just switched into.
  useLayoutEffect(() => {
    if (!isOpen) return;
    if (resolvedIds.size === 0) return;
    const body = introRef.current?.closest('.modal__body') ?? null;
    const active = document.activeElement;
    if (active instanceof HTMLElement && body?.contains(active)) return;
    const next =
      (totalRemaining > 0
        ? body?.querySelector<HTMLElement>('.missed-tasks__actions button')
        : null) ?? introRef.current;
    next?.focus({ preventScroll: true });
  }, [isOpen, totalRemaining, resolvedIds]);

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

  // Delete a task that's no longer relevant. The ONLY irreversible action in
  // this dialog (done / backlog / today / tomorrow are all reversible), so it
  // confirms first — matching how task deletion is gated elsewhere in the app.
  const deleteTask = useCallback(
    async (task: Task): Promise<void> => {
      if (
        !window.confirm(
          t('dialogs.dayStartReview.confirmDelete', { title: task.title }),
        )
      ) {
        return;
      }
      setBusy(true);
      try {
        await invoke<void>('delete_task', { id: task.id, listId: task.list_id });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.dayStartReview.announceDeleted', { title: task.title }),
        );
        invalidateData();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('delete_task failed', err);
      } finally {
        setBusy(false);
      }
    },
    [t, announce, invalidateData],
  );

  // The row title + date were plain <span>s inside the role="application" Modal,
  // so NVDA's focus-mode traversal (it only stops on focusable elements) skipped
  // them — the user had to drop into object navigation to read which task a row
  // was. Make the title container focusable with an aria-label carrying the whole
  // row (title, priority, date); the child spans go aria-hidden so it isn't read
  // twice. Same idea as FocusableNote, kept inline so the visible priority dot +
  // date styling survive for sighted users.
  const rowAriaLabel = useCallback(
    (task: Task, dateText: string | null): string => {
      const parts = [task.title];
      const pk = priorityI18nKey(task.priority);
      if (pk) parts.push(t(pk));
      if (dateText) parts.push(dateText);
      return parts.join(', ');
    },
    [t],
  );

  /**
   * Move a lapsed deadline to today, keeping the time of day.
   *
   * The section's only other way out of an overdue deadline was Backlog, which
   * clears the deadline AND the scheduling — everything, when the answer is
   * usually "it is still due, just not yesterday".
   *
   * The TIME is kept on purpose: a deadline of 14:00 that slipped is still a
   * 14:00 deadline, and dropping it to "sometime today" would quietly discard
   * what the user wrote. The SCHEDULING is left alone for the same reason —
   * what lapsed is the deadline, not the plan.
   *
   * One task, no cascade over subtasks, matching `backToBacklog` right beside
   * it. Two buttons in one row where one drags the children along and the
   * other does not would be worse than either rule on its own.
   */
  const deadlineToToday = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      try {
        const updated: Task = {
          ...deadlineMovedToToday(task),
          updated_at: new Date().toISOString(),
        };
        await invoke<Task>('update_task', { task: updated });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.dayStartReview.deadlines.announceDeadlineToday', {
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
        // What the SOURCE can actually do with "move this one to that day".
        // iOS Reminders keeps the due date as the series anchor, so an
        // arbitrary day written to a repeating reminder does not stick — and it
        // used to not stick SILENTLY, this dialog announcing a move that never
        // happened. Where the source cannot move one occurrence, the series
        // advances by a step instead and the announcement says so.
        let advanced = false;
        for (const target of targets) {
          const move = occurrenceMoveTarget(
            target,
            newDate,
            taskListById.get(target.list_id)?.task_capabilities
              ?.reschedule_single_occurrence ?? true,
          );
          if (move.advanced) advanced = true;
          await invoke<Task>('update_task', {
            task: {
              ...target,
              scheduled_date: move.date,
              updated_at: now,
            },
          });
        }
        setResolvedIds((s) => {
          const next = new Set(s);
          targets.forEach((task) => next.add(task.id));
          return next;
        });
        // Says what HAPPENED, not what was asked for.
        const base = advanced
          ? t('dialogs.dayStartReview.carryOver.announceAdvancedSeries')
          : t(announcementKey, { title: root.title });
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
    [tasks, taskListById, effectiveForList, t, announce, invalidateData],
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
  // Reminders are a valid reason to stay open even with no actionable
  // rows, so a reminders-only review isn't dismissed here.
  //
  // NEVER while useTasks is still loading: opening a task editor from a
  // review row unmounts this dialog (the dialog stack renders only its
  // top), and any mutation in the editor clears the useTasks SWR cache —
  // so the RE-mount after closing the editor briefly sees zero tasks.
  // Without the loading gate that instant read as "nothing to show" and
  // silently killed the review the user was mid-way through (the mobile
  // modal's twin guard has always waited for the fetch).
  if (
    !tasksLoading &&
    totalRemaining === 0 &&
    resolvedIds.size === 0 &&
    !hasReminders
  ) {
    onClose();
    return null;
  }

  // The intro copy follows the same switch: the default hint promises per-row
  // decisions and bulk actions that don't exist in a reminders-only review.
  const hintText = t(
    remindersOnly
      ? 'dialogs.dayStartReview.hintRemindersOnly'
      : 'dialogs.dayStartReview.hint',
  );

  return (
    <Modal
      isOpen={isOpen}
      // Escape / the × snooze a real review, but merely acknowledge a
      // reminders-only one (see `remindersOnly` above).
      onClose={remindersOnly ? onClose : snoozeLater}
      title={t('dialogs.dayStartReview.title')}
      className="modal--form modal--day-start-review"
      initialFocusRef={introRef}
      // A stray click OUTSIDE the dialog must not defer the whole review —
      // this is the day's work list, and the backdrop-click was read as
      // "remind me later" (4h snooze), silently for a mouse user. Deferring
      // stays an EXPLICIT action: the "remind me later" button, or Escape
      // (which routes to the same announced snooze via onClose).
      dismissOnBackdrop={false}
    >
      <p
        ref={introRef}
        tabIndex={-1}
        className="form__hint"
        // The dialog opens with focus here (initialFocusRef). A bare focusable
        // <p> computes no accessible name, so NVDA announced only the dialog
        // title and fell silent — the hint that explains the dialog's purpose
        // was never spoken. Mirror the text into aria-label (same technique as
        // FocusableNote and the row titles) so it is read on open. Kept
        // tabIndex={-1}: a programmatic landing spot, not a Tab stop.
        aria-label={hintText}
      >
        {hintText}
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
            {remainingOverdue.map((task) => {
              const dateText = task.deadline_date
                ? t('dialogs.dayStartReview.deadlines.dateLabel', {
                    date: formatIsoDate(task.deadline_date, i18n.language),
                  })
                : null;
              return (
                <li key={task.id} className="missed-tasks__row">
                  <div
                    className="missed-tasks__title"
                    tabIndex={0}
                    aria-label={rowAriaLabel(task, dateText)}
                  >
                    <span className="missed-tasks__name" aria-hidden="true">
                      {task.title}
                    </span>
                    {priorityMarker(task.priority) && (
                      <span className="missed-tasks__priority" aria-hidden="true">
                        {priorityMarker(task.priority)}
                      </span>
                    )}
                    {dateText && (
                      <span className="missed-tasks__deadline" aria-hidden="true">
                        {dateText}
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
                      onClick={() => void deadlineToToday(task)}
                      aria-disabled={busy || undefined}
                    >
                      {t('dialogs.dayStartReview.deadlines.actions.today')}
                    </button>
                    <button
                      type="button"
                      className="form__action"
                      onClick={() => void backToBacklog(task)}
                      aria-disabled={busy || undefined}
                    >
                      {t('dialogs.dayStartReview.deadlines.actions.backlog')}
                    </button>
                    <button
                      type="button"
                      className="form__action form__action--danger"
                      onClick={() => void deleteTask(task)}
                      aria-disabled={busy || undefined}
                    >
                      {t('dialogs.dayStartReview.delete')}
                    </button>
                  </div>
                </li>
              );
            })}
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

      {hasReminders && (
        <section className="day-start-review__section day-start-review__section--reminders">
          <h3 className="day-start-review__heading">
            {t('dialogs.dayStartReview.reminders.heading')}
          </h3>
          {reminderGroups.map((group) =>
            group.tasks.length === 0 ? null : (
              <div key={group.key} className="day-start-review__reminder-group">
                <p className="day-start-review__reminder-summary">
                  {group.summary}
                </p>
                {/* No aria-label here — the preceding <p> already names
                    the group, so labelling the <ul> too would read the
                    summary twice. */}
                <ul className="missed-tasks__list">

                  {group.tasks.map((task) => (
                    <li key={task.id} className="missed-tasks__row">
                      <button
                        type="button"
                        className="day-start-review__reminder-task"
                        onClick={() => openTaskDialog(task)}
                        aria-label={`${task.title}, ${group.why(task)}`}
                      >
                        <span aria-hidden="true">{task.title}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            ),
          )}
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
            {remainingSlipped.map((task) => {
              const dateText = task.scheduled_date
                ? t('dialogs.dayStartReview.carryOver.dateLabel', {
                    date: formatIsoDate(task.scheduled_date, i18n.language),
                  })
                : null;
              return (
                <li key={task.id} className="missed-tasks__row">
                  <div
                    className="missed-tasks__title"
                    tabIndex={0}
                    aria-label={rowAriaLabel(task, dateText)}
                  >
                    <span className="missed-tasks__name" aria-hidden="true">
                      {task.title}
                    </span>
                    {priorityMarker(task.priority) && (
                      <span className="missed-tasks__priority" aria-hidden="true">
                        {priorityMarker(task.priority)}
                      </span>
                    )}
                    {dateText && (
                      <span className="missed-tasks__deadline" aria-hidden="true">
                        {dateText}
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
                    <button
                      type="button"
                      className="form__action form__action--danger"
                      onClick={() => void deleteTask(task)}
                      aria-disabled={busy || undefined}
                    >
                      {t('dialogs.dayStartReview.delete')}
                    </button>
                  </div>
                </li>
              );
            })}
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
        {remindersOnly ? (
          <button
            type="button"
            className="form__action form__action--primary"
            onClick={onClose}
          >
            {t('dialogs.dayStartReview.acknowledge')}
          </button>
        ) : (
          <button type="button" className="form__action" onClick={snoozeLater}>
            {t('dialogs.dayStartReview.snooze')}
          </button>
        )}
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
