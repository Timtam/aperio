import { useCallback, useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, AppState } from 'react-native';

import {
  actionableDescendants,
  filterCarriedOver,
  filterDeadlinePinTargets,
  filterOverdue,
  shouldFireToday,
  todayIsoKey,
} from '@aperio/shared';
import type { Task } from '@aperio/shared';
import i18n from '../../i18n';

import { getTasks, listTaskLists, updateTask } from '../api/client';
import { readFiredDayKey, writeFiredDayKey } from './dayStartFired';
import { isDayStartReviewSnoozed } from './dayStartSnooze';
import { effectiveForList, readTaskBehaviour, type TaskBehaviour } from './taskBehaviour';
import { useTaskStore } from './taskStoreContext';

// The mobile day-start checks — the screen-reader-first twin of the desktop's
// DeadlinePinChecker + DayStartReviewChecker. The desktop fires from a live
// minute-poller; iOS suspends background JS, so mobile runs the checks on launch
// + every foreground-resume (the same model as the reminder rescheduler), gated
// by the synced dayStartTrigger pref + a per-device per-slot fire-marker so a
// day's batch runs at most once. All best-effort + silent on failure.

/** Every task across the user's lists (for the cross-list deadline-pin). */
async function loadAllTasks(): Promise<Task[]> {
  const lists = await listTaskLists();
  const per = await Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[])));
  return per.flat();
}

/** Tasks across the given lists — the day-start review's scope mirrors the
 *  desktop's (it reads `useTasks`, i.e. the SELECTED lists), so the checker's
 *  decision and the modal's re-derivation see the same set. */
async function loadTasksForLists(ids: string[]): Promise<Task[]> {
  const per = await Promise.all(ids.map((id) => getTasks(id).catch(() => [] as Task[])));
  return per.flat();
}

/**
 * Silent "by"-deadline auto-pin: tasks whose deadline is today (and aren't
 * already scheduled for today) get pinned to today so they surface on today's
 * calendar lanes. Gated by dayStartTrigger + the 'deadlinePin' fire-marker. The
 * marker is written BEFORE applying (idempotent — a partial run isn't re-fired).
 */
async function runDeadlinePin(invalidateData: () => void): Promise<void> {
  const behaviour = await readTaskBehaviour();
  const todayKey = todayIsoKey();
  const fired = await readFiredDayKey('deadlinePin');
  if (!shouldFireToday(behaviour.dayStartTrigger, fired, todayKey)) return;
  const all = await loadAllTasks();
  await writeFiredDayKey('deadlinePin', todayKey);
  const targets = filterDeadlinePinTargets(all);
  if (targets.length === 0) return;
  for (const task of targets) {
    // Pin to today; leave scheduled_time untouched ("by 14:30" ≠ "at 14:30").
    await updateTask({ ...task, scheduled_date: todayKey });
  }
  AccessibilityInfo.announceForAccessibility(
    i18n.t('dialogs.deadlinePin.announce', { count: targets.length }),
  );
  invalidateData();
}

/**
 * Silent carry-over batch (one action) — the mobile twin of the desktop
 * runAutoCarryOverBatch. Collects every slipped root plus, when cascade is on
 * for THAT root's list, its actionable descendants, and shifts each one's
 * scheduled_date (today or null). Announces the ROOT count (descendants are an
 * implementation detail). No visual undo-toast: mobile has no toast surface yet,
 * and the screen-reader announce — the channel that matters here — is covered.
 */
async function runAutoCarryOverBatch(
  action: 'today' | 'backlog',
  slippedRoots: Task[],
  allTasks: Task[],
  behaviour: TaskBehaviour,
): Promise<void> {
  const collected = new Map<string, Task>();
  for (const root of slippedRoots) {
    collected.set(root.id, root);
    if (!effectiveForList(behaviour, root.list_id).cascade) continue;
    for (const desc of actionableDescendants(root.id, allTasks)) {
      collected.set(desc.id, desc);
    }
  }
  const targets = [...collected.values()];
  if (targets.length === 0) return;
  const newDate = action === 'today' ? todayIsoKey() : null;
  // Sequential so a first-row failure surfaces without a half-applied family.
  for (const task of targets) {
    await updateTask({ ...task, scheduled_date: newDate });
  }
  AccessibilityInfo.announceForAccessibility(
    i18n.t(
      action === 'today'
        ? 'dialogs.dayStartReview.carryOver.autoToday'
        : 'dialogs.dayStartReview.carryOver.autoBacklog',
      { count: slippedRoots.length },
    ),
  );
}

/**
 * The unified day-start review gate — the mobile twin of DayStartReviewChecker.
 * Reads overdue (lapsed deadline) + slipped (lapsed scheduled day) across the
 * SELECTED lists, splits slipped rows by each list's carry-over default, runs
 * the silent batch for the today/backlog lists, and opens the review modal iff
 * there's still something to decide (an overdue row or a slipped row whose list
 * voted 'ask'). Gated by dayStartTrigger + the 'dayStartReview' fire-marker +
 * the 4-hour snooze. A snooze bail does NOT mark fired — the gate re-runs once
 * the snooze expires.
 */
async function runDayStartReview(
  selectedIds: string[],
  invalidateData: () => void,
  openReview: () => void,
): Promise<void> {
  const behaviour = await readTaskBehaviour();
  const todayKey = todayIsoKey();
  const fired = await readFiredDayKey('dayStartReview');
  if (!shouldFireToday(behaviour.dayStartTrigger, fired, todayKey)) return;
  // Snooze respects the user's "remind me later" choice. Do NOT mark fired —
  // the next eligible tick should run the gate once the snooze expires.
  if (await isDayStartReviewSnoozed()) return;
  if (selectedIds.length === 0) {
    // Nothing in scope; still record the fire so we don't keep re-checking.
    await writeFiredDayKey('dayStartReview', todayKey);
    return;
  }

  const all = await loadTasksForLists(selectedIds);
  // Mark fired BEFORE applying — even an empty day records the fire (the gate's
  // only job is "review for this day"); a partial run isn't re-fired.
  await writeFiredDayKey('dayStartReview', todayKey);

  const overdue = filterOverdue(all);
  const slipped = filterCarriedOver(all, {
    cascadeEnabledFor: (listId) => effectiveForList(behaviour, listId).cascade,
  });

  // Split slipped rows by each list's carry-over default: 'today' / 'backlog'
  // run silently, 'ask' surfaces in the modal. A mix produces a hybrid.
  const askRows: Task[] = [];
  const todayRows: Task[] = [];
  const backlogRows: Task[] = [];
  for (const row of slipped) {
    const def = effectiveForList(behaviour, row.list_id).carryOverDefault;
    if (def === 'today') todayRows.push(row);
    else if (def === 'backlog') backlogRows.push(row);
    else askRows.push(row);
  }

  if (todayRows.length > 0) {
    await runAutoCarryOverBatch('today', todayRows, all, behaviour);
  }
  if (backlogRows.length > 0) {
    await runAutoCarryOverBatch('backlog', backlogRows, all, behaviour);
  }
  if (todayRows.length + backlogRows.length > 0) invalidateData();

  // Open the modal iff there's still a decision to make. Bump the data version
  // first so the modal's own `useTasks` re-reads from the bridge rather than a
  // possibly-stale warm cache — the checker read the bridge directly (a
  // separate fan-out), so this keeps the modal authoritative over what it acts
  // on and makes its loading-guard meaningful.
  if (overdue.length + askRows.length > 0) {
    invalidateData();
    openReview();
  }
}

// One run at a time across launch + the foreground listener.
let inFlight = false;

/**
 * Mount once inside the TaskStore provider: run the day-start checks on launch
 * (once the catalog + selection have hydrated) + every foreground-resume (the
 * latter catches a date rollover while away). Returns the review-modal state so
 * the mounting component can render the modal — the modal must overlay any tab,
 * and the checker lives above the navigator, so a navigation screen would be the
 * wrong tool; an app-level modal driven by this flag is the fit.
 */
export function useDayStartChecks(): { reviewOpen: boolean; closeReview: () => void } {
  const { invalidateData, selectedTaskListIds, taskListsLoading } = useTaskStore();
  const [reviewOpen, setReviewOpen] = useState(false);

  // The AppState listener registers once; it reads the live selection +
  // catalog-loading state through refs so it never needs re-subscribing when
  // either changes.
  const selectionRef = useRef(selectedTaskListIds);
  selectionRef.current = selectedTaskListIds;
  const loadingRef = useRef(taskListsLoading);
  loadingRef.current = taskListsLoading;

  const openReview = useCallback(() => setReviewOpen(true), []);
  const closeReview = useCallback(() => setReviewOpen(false), []);

  const run = useCallback(async () => {
    if (inFlight) return;
    inFlight = true;
    try {
      await runDeadlinePin(invalidateData);
      // The review reads the SELECTED lists, so it must wait for the store to
      // hydrate (an empty pre-hydration selection would mark the day fired with
      // nothing to review). The catalog-ready effect below re-runs us then.
      if (!loadingRef.current) {
        await runDayStartReview([...selectionRef.current], invalidateData, openReview);
      }
    } catch {
      // Best-effort — a bridge hiccup must never crash launch/foreground.
    } finally {
      inFlight = false;
    }
  }, [invalidateData, openReview]);

  // Launch + catalog-ready: fire once the task-list catalog + selection have
  // hydrated (taskListsLoading flips false). Re-firing here is harmless — the
  // inFlight guard + the fire-marker make repeat runs no-ops.
  useEffect(() => {
    if (taskListsLoading) return;
    void run();
  }, [taskListsLoading, run]);

  // Foreground-resume: catches a date rollover (or a snooze expiry) while away.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') void run();
    });
    return () => sub.remove();
  }, [run]);

  return { reviewOpen, closeReview };
}
