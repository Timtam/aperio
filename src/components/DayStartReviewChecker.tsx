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
} from '../state/TaskCascadeProvider';
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
 * Behaviour:
 *
 *  - When `carryOverDefault === 'today' | 'backlog'` the carry-over
 *    half is applied silently in a batch BEFORE we consider opening
 *    a dialog. The Settings → Tasks "Übernahme-Standard" preference
 *    is honoured exactly as the old CarryOverChecker did.
 *  - The dialog opens iff there is at least one row left to show
 *    after the silent batch — either overdue rows or, in `ask` mode,
 *    the carry-over rows themselves.
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
  const {
    enabled: cascadeEnabled,
    carryOverDefault,
    dayStartTrigger,
    hydrating,
  } = useTaskCascadeEnabled();
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
    const slipped = filterCarriedOver(tasks, { cascadeEnabled });
    // Even on an empty day we record the fire — the gate's only job
    // is "review for this day". If new slipped / overdue rows appear
    // later (sync, manual edit), this tick wouldn't have caught
    // them either; the user explicitly re-running the review later
    // is the answer there.
    firedRef.current = todayKey;
    writeFiredDayKey('dayStartReview', todayKey);

    const wantsAuto =
      carryOverDefault !== 'ask' && slipped.length > 0;
    if (wantsAuto) {
      // Apply the carry-over batch silently, then re-decide whether
      // the dialog still needs to open (only when there are overdue
      // rows left to address — the carry-over half just got
      // handled).
      void (async () => {
        await runAutoCarryOverBatch({
          action: carryOverDefault,
          slippedRoots: slipped,
          allTasks: tasks,
          cascadeEnabled,
          announce,
          t,
          invalidateData,
        });
        if (overdue.length > 0) openDayStartReview();
      })();
      return;
    }

    if (overdue.length + slipped.length === 0) return;
    openDayStartReview();
  }, [
    loading,
    hydrating,
    tasks,
    cascadeEnabled,
    carryOverDefault,
    dayStartTrigger,
    todayKey,
    dialogMode.kind,
    openDayStartReview,
    invalidateData,
    announce,
    t,
  ]);

  return null;
}

/**
 * Apply a silent carry-over batch action. Collects every slipped row
 * plus, when cascade-coupling is on, its actionable descendants — the
 * same target set the dialog's bulk buttons would touch.
 *
 * Unchanged from the standalone CarryOverChecker version other than
 * the relocation. The announce key still lives under
 * `dialogs.dayStartReview.carryOver.auto*` so the message text is
 * specific ("automatisch auf heute übernommen") rather than generic.
 */
async function runAutoCarryOverBatch(args: {
  action: Exclude<CarryOverDefault, 'ask'>;
  slippedRoots: Task[];
  allTasks: Task[];
  cascadeEnabled: boolean;
  announce: (message: string) => void;
  t: (key: string, values?: Record<string, unknown>) => string;
  invalidateData: () => void;
}): Promise<void> {
  const { action, slippedRoots, allTasks, cascadeEnabled } = args;
  const collected = new Map<string, Task>();
  for (const root of slippedRoots) {
    collected.set(root.id, root);
    if (!cascadeEnabled) continue;
    for (const desc of actionableDescendants(root.id, allTasks)) {
      collected.set(desc.id, desc);
    }
  }
  const targets = [...collected.values()];
  if (targets.length === 0) return;

  const newDate = action === 'today' ? todayIsoKey() : null;
  const now = new Date().toISOString();
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
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn('day-start review carry-over batch failed', err);
  }
}
