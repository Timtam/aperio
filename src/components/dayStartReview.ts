import type { Task } from '../api/types';
import { todayIsoKey } from '../intl/taskDay';

/**
 * Helpers for the day-start review flow.
 *
 * One dialog now hosts what used to be two: the deadline-overrun list
 * (formerly MissedTasksDialog) and the schedule-slip list (formerly
 * CarryOverDialog). Both kinds of slips share a single trigger gate
 * (see {@link DayStartReviewChecker}) so the user gets one prompt at
 * day-start rather than a stacked sequence — the old setup deferred
 * one dialog to the next minute tick because the modal-stacking guard
 * wouldn't let both open at once.
 *
 * The two selectors below are still distinct because the underlying
 * fields ARE distinct:
 *
 *  - {@link filterOverdue} picks tasks whose **deadline** has lapsed
 *    ("you said you'd be done by then, what now?")
 *  - {@link filterCarriedOver} picks tasks whose **scheduled day** has
 *    lapsed but no deadline has been crossed ("you said you'd work on
 *    it yesterday, where should it go now?")
 *
 * The dialog renders them as two sections with different per-row
 * actions: deadlines get Done / Back-to-Backlog; carry-over gets
 * Today / Tomorrow / Backlog / Done. Mixing those into one row would
 * make the buttons ambiguous (Today on a deadline row would silently
 * overwrite the deadline date, which the user didn't ask for).
 */

/**
 * "Overdue" = has a deadline_date strictly before today AND is not
 * already completed/cancelled. `scheduled_date` alone doesn't count
 * — that's a planning hint, not a missed commitment.
 */
export function filterOverdue(tasks: Task[]): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
    }
    return task.deadline_date < today;
  });
}

/**
 * Picks tasks with a `scheduled_date` strictly before today, still in
 * an actionable status (`open` or `in_progress`).
 *
 * Cascade-coupling is resolved per task list via the optional
 * `cascadeEnabledFor` callback — so a user who set "cascade on" for
 * the work list and "cascade off" for the hobby list gets both
 * behaviours in the same run. When the callback is omitted, cascade
 * is treated as off for every list (the historical "no cascade"
 * default that simpler callers and the unit tests use).
 *
 * For a list with cascade on: a slipped task is hidden if any
 * ancestor in the same list is also slipped — the user only decides
 * at the root of each slipped subtree, and the dialog's action
 * handlers propagate the chosen verdict (Heute / Morgen / Backlog /
 * Erledigt) to the actionable descendants. An "orphaned" slipped
 * subtask whose parent itself isn't slipped still surfaces as its
 * own row.
 *
 * For a list with cascade off: every slipped task appears as its own
 * row regardless of hierarchy.
 *
 * Cross-section dedup: tasks that ALSO appear in the overdue list
 * (deadline strictly before today AND scheduled_date strictly before
 * today) are dropped here, because the deadline is the bigger lever —
 * the dialog shows them in the deadline section instead, where Done
 * / Back-to-Backlog covers the same outcomes. Without this filter a
 * task could appear in both sections and the user would have to
 * settle it twice.
 */
export function filterCarriedOver(
  tasks: Task[],
  options?: {
    /** Resolve per-list cascade preference. Parent and child tasks
     *  live in the same list (invariant from #98), so walking the
     *  ancestor chain calls this with one list id per hop and
     *  short-circuits the moment a non-cascading list is hit. */
    cascadeEnabledFor?: (listId: string) => boolean;
  },
): Task[] {
  const today = todayIsoKey();
  const overdueIds = new Set(filterOverdue(tasks).map((t) => t.id));
  const slipped = tasks.filter((task) => {
    if (!task.scheduled_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
    }
    if (overdueIds.has(task.id)) return false;
    return task.scheduled_date < today;
  });

  const cascadeFor = options?.cascadeEnabledFor;
  if (!cascadeFor) return slipped;

  const slippedIds = new Set(slipped.map((t) => t.id));
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const hasSlippedAncestor = (task: Task): boolean => {
    // Cascade-coupling for THIS task's list decides whether to
    // suppress it when an ancestor is slipped. If the task's own
    // list has cascade off, the row appears even when the parent is
    // slipped — same as the historical "cascade off" behaviour.
    if (!cascadeFor(task.list_id)) return false;
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

// ── Snooze plumbing ────────────────────────────────────────────────────

const SNOOZE_KEY = 'aperio.dayStartReview.snoozeUntil';

/** Legacy keys from the two-dialog era. We still respect them on read
 *  so a snooze the user set on the old build doesn't suddenly stop
 *  being effective after the upgrade — the 4-hour window expires by
 *  itself and writes from then on use the unified key. Removed in a
 *  future release once the legacy windows have all aged out. */
const LEGACY_MISSED_KEY = 'aperio.missedTasks.snoozeUntil';
const LEGACY_CARRY_OVER_KEY = 'aperio.carryOver.snoozeUntil';

/**
 * Suppress the unified review for `hours` hours. Single key now —
 * the two-dialog era kept separate ones so snoozing deadlines didn't
 * silence carry-over; with a single combined dialog that distinction
 * no longer maps to a UI affordance.
 */
export function snoozeDayStartReview(hours: number): void {
  try {
    const until = Date.now() + hours * 60 * 60 * 1000;
    localStorage.setItem(SNOOZE_KEY, String(until));
  } catch {
    // Storage unavailable; dialog will simply re-appear on the next
    // poller tick (or the next app start).
  }
}

export function isDayStartReviewSnoozed(): boolean {
  try {
    for (const key of [SNOOZE_KEY, LEGACY_MISSED_KEY, LEGACY_CARRY_OVER_KEY]) {
      const raw = localStorage.getItem(key);
      if (!raw) continue;
      const until = Number.parseInt(raw, 10);
      if (Number.isNaN(until)) continue;
      if (Date.now() < until) return true;
    }
    return false;
  } catch {
    return false;
  }
}
