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
import {
  useTaskCascadeEnabled,
  type CarryOverDefault,
  type EffectiveListSettings,
} from '../state/TaskCascadeProvider';
import { useToast } from '../state/ToastProvider';
import { useTasks } from '../state/useTasks';
import {
  actionableDescendants,
  filterCarriedOver,
  filterOverdue,
  isDayStartReviewSnoozed,
} from './dayStartReview';

/**
 * Day-start gate for the unified review (DESIGN.md § 9.5).
 *
 * Replaces the old MissedTasksChecker + CarryOverChecker pair. Both
 * checkers used to fire independently, but the modal-stacking guard
 * (`dialogMode.kind !== 'none'`) meant whichever opened second got
 * deferred to the next minute tick — the user saw deadlines first
 * and only later realised there was also a carry-over list. One
 * gate, one dialog, both sections.
 *
 * Per-list overrides: the cascade-coupling and carry-over-default
 * preferences resolve per task list via
 * `useTaskCascadeEnabled().effectiveForList(listId)`. The silent
 * auto-batch path groups slipped tasks by their list and applies
 * each list's chosen action — Work might "ask", Hobby might
 * "auto-today", and the user gets a dialog for the Work rows while
 * the Hobby rows quietly shift without prompting.
 *
 * Re-trigger semantics: a persistent `firedDayKey` in localStorage
 * (slot `dayStartReview`) prevents re-firing for a day the user
 * already saw. The `dayStartTrigger` preference (immediately at
 * midnight, fixed morning hour, or app-start only) further gates
 * *when* on the new day. See `shouldFireToday`.
 *
 * Snooze: the dialog's "Später erinnern" button writes a 4-hour
 * suppress flag. A snooze bail does NOT mark `firedRef` — the next
 * tick re-runs the gate once the snooze expires.
 *
 * Dialog guard: if any modal is already open (e.g. the user is
 * editing a task), we skip this tick. The poller will try again.
 * This avoids silently rewriting fields under an open editor.
 */
export function DayStartReviewChecker() {
  const { tasks, loading } = useTasks();
  const {
    mode: dialogMode,
    openDayStartReview,
    invalidateData,
  } = useDialogState();
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const { effectiveForList, dayStartTrigger, hydrating } =
    useTaskCascadeEnabled();
  const { showToast } = useToast();
  const todayKey = useCurrentDayKey();
  // Persistent fire marker — hydrated from localStorage on mount so a
  // mid-day app restart doesn't re-run the silent batch (and
  // re-announce) for a day already processed.
  const firedRef = useRef<string | null>(readFiredDayKey('dayStartReview'));

  useEffect(() => {
    // Wait for both the task catalog and the preferences round-trip
    // — otherwise a default-`ask` from the pre-hydration state would
    // open the dialog even for users who opted into auto-today /
    // auto-backlog.
    if (loading || hydrating) return;
    if (!shouldFireToday(dayStartTrigger, firedRef.current, todayKey)) {
      return;
    }
    // Don't pile a second day-start dialog (or silent batch) on top
    // of whatever the user already has open. Tick again later when
    // the modal closes.
    if (dialogMode.kind !== 'none') return;
    // Snooze respects the user's "Später erinnern" choice. Do NOT
    // mark fired — when the snooze expires, the next tick should
    // run the gate properly.
    if (isDayStartReviewSnoozed()) return;

    const overdue = filterOverdue(tasks);
    const slipped = filterCarriedOver(tasks, {
      cascadeEnabledFor: (listId) => effectiveForList(listId).cascade,
    });
    // Even on an empty day we record the fire — the gate's only job
    // is "review for this day". If new slipped / overdue rows appear
    // later (sync, manual edit), this tick wouldn't have caught
    // them either; the user explicitly re-running the review later
    // is the answer there.
    firedRef.current = todayKey;
    writeFiredDayKey('dayStartReview', todayKey);

    // Group slipped tasks by list and split by each list's carry-over
    // default. Tasks in lists set to 'ask' end up in the dialog; tasks
    // in 'today' / 'backlog' lists run through the silent batch with
    // the appropriate action. A mix of lists with different defaults
    // produces a hybrid — some rows handled silently, others surfaced
    // for explicit review.
    const askRows: Task[] = [];
    const todayRows: Task[] = [];
    const backlogRows: Task[] = [];
    for (const row of slipped) {
      const eff = effectiveForList(row.list_id);
      if (eff.carryOverDefault === 'today') todayRows.push(row);
      else if (eff.carryOverDefault === 'backlog') backlogRows.push(row);
      else askRows.push(row);
    }

    const hasSilentWork = todayRows.length > 0 || backlogRows.length > 0;
    if (hasSilentWork) {
      void (async () => {
        // Run each batch in its own pass so a failure on one half
        // (e.g. an offline iCloud account) doesn't block the other.
        // Both share the same `effectiveForList` so each batch's
        // cascade decision honours the originating row's list.
        if (todayRows.length > 0) {
          await runAutoCarryOverBatch({
            action: 'today',
            slippedRoots: todayRows,
            allTasks: tasks,
            effectiveForList,
            announce,
            t,
            invalidateData,
            showToast,
          });
        }
        if (backlogRows.length > 0) {
          await runAutoCarryOverBatch({
            action: 'backlog',
            slippedRoots: backlogRows,
            allTasks: tasks,
            effectiveForList,
            announce,
            t,
            invalidateData,
            showToast,
          });
        }
        // Dialog opens iff there's still something to talk about —
        // either an overdue row or a slipped row whose list voted
        // 'ask'.
        if (overdue.length + askRows.length > 0) openDayStartReview();
      })();
      return;
    }

    // No silent work: pure ask-mode. Open the dialog if there's
    // anything in either section, otherwise stay quiet.
    if (overdue.length + askRows.length === 0) return;
    openDayStartReview();
  }, [
    loading,
    hydrating,
    tasks,
    effectiveForList,
    dayStartTrigger,
    todayKey,
    dialogMode.kind,
    openDayStartReview,
    invalidateData,
    announce,
    showToast,
    t,
  ]);

  return null;
}

/**
 * Apply a silent carry-over batch action. Collects every slipped row
 * plus, when cascade-coupling is on for THAT row's list, its
 * actionable descendants — the same target set the dialog's bulk
 * buttons would touch, with per-list cascade respected.
 *
 * After a successful batch we surface a visible toast with an Undo
 * button. The Undo handler re-applies each task's original
 * `scheduled_date` (captured before the batch) so the user can
 * back out the silent change. We restore only that one field —
 * not the whole task snapshot — to minimise the blast radius of
 * a concurrent edit that happens during the 10-second toast
 * window.
 *
 * Single-action per call — the caller splits rows by action and
 * runs us twice when the user has mixed today/backlog defaults
 * across lists.
 */
async function runAutoCarryOverBatch(args: {
  action: Exclude<CarryOverDefault, 'ask'>;
  slippedRoots: Task[];
  allTasks: Task[];
  effectiveForList: (listId: string) => EffectiveListSettings;
  announce: (message: string) => void;
  t: (key: string, values?: Record<string, unknown>) => string;
  invalidateData: () => void;
  showToast: (input: {
    message: string;
    undo?: { label?: string; action: () => Promise<void> | void };
    durationMs?: number;
  }) => string;
}): Promise<void> {
  const { action, slippedRoots, allTasks, effectiveForList } = args;
  const collected = new Map<string, Task>();
  for (const root of slippedRoots) {
    collected.set(root.id, root);
    if (!effectiveForList(root.list_id).cascade) continue;
    for (const desc of actionableDescendants(root.id, allTasks)) {
      collected.set(desc.id, desc);
    }
  }
  const targets = [...collected.values()];
  if (targets.length === 0) return;

  const newDate = action === 'today' ? todayIsoKey() : null;
  const now = new Date().toISOString();
  // Snapshot the prior scheduled_date for every target BEFORE we
  // mutate. The undo handler replays this map, so the toast keeps a
  // closure over it for its 10-second lifetime even if `targets`
  // (the live `tasks` reference) has moved on by the time the user
  // clicks.
  const originalSchedules: Array<{
    task: Task;
    previousScheduledDate: string | null;
  }> = targets.map((task) => ({
    task,
    previousScheduledDate: task.scheduled_date ?? null,
  }));
  try {
    await Promise.all(
      targets.map((task) =>
        invoke<Task>('update_task', {
          task: {
            ...task,
            scheduled_date: newDate,
            updated_at: now,
          },
        }),
      ),
    );
    // The count we announce is the number of *root* rows — descendants
    // under coupling are implementation detail and shouldn't pad the
    // human-facing count.
    const announceKey =
      action === 'today'
        ? 'dialogs.dayStartReview.carryOver.autoToday'
        : 'dialogs.dayStartReview.carryOver.autoBacklog';
    args.announce(args.t(announceKey, { count: slippedRoots.length }));
    args.invalidateData();

    // Surface a visible Undo handle. The screen-reader-only announce
    // call above already told assistive tech what happened; the
    // toast provides the matching visual channel + an actionable
    // button. The toast's own `role="status" aria-live="polite"`
    // means SR users hear the toast text too, but the announcer's
    // shorter message is what hits first.
    const toastKey =
      action === 'today'
        ? 'dialogs.dayStartReview.carryOver.autoTodayToast'
        : 'dialogs.dayStartReview.carryOver.autoBacklogToast';
    args.showToast({
      message: args.t(toastKey, { count: slippedRoots.length }),
      undo: {
        action: async () => {
          // Replay the snapshot in parallel. We restore only the
          // single `scheduled_date` field; anything else the user
          // may have edited in the meantime stays as-is.
          const undoNow = new Date().toISOString();
          await Promise.all(
            originalSchedules.map(({ task, previousScheduledDate }) =>
              invoke<Task>('update_task', {
                task: {
                  ...task,
                  scheduled_date: previousScheduledDate,
                  updated_at: undoNow,
                },
              }),
            ),
          );
          args.invalidateData();
          args.announce(
            args.t('dialogs.dayStartReview.carryOver.undoAnnounce', {
              count: slippedRoots.length,
            }),
          );
        },
      },
    });
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn('day-start review carry-over batch failed', err);
  }
}
