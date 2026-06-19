import { todayIsoKey } from './taskDay';
import type { Task } from './types';

// Pure day-start-review + deadline-pin selectors + the fire-gate, shared by the
// desktop checkers and the mobile day-start checks. No platform deps (storage /
// timers live in the platform layer). See the desktop dayStartReview.ts /
// deadlinePinTargets.ts / useCurrentDayKey.ts for the original prose.

/**
 * "Overdue" = a `deadline_date` strictly before today AND not already
 * completed/cancelled. `scheduled_date` alone doesn't count — that's a planning
 * hint, not a missed commitment.
 */
export function filterOverdue(tasks: Task[]): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    return task.deadline_date < today;
  });
}

/**
 * Tasks with a `scheduled_date` strictly before today, still actionable (`open`
 * / `in_progress`), and NOT already in the overdue list (the deadline is the
 * bigger lever, shown in that section). When `cascadeEnabledFor` is given, a
 * slipped task in a cascading list is hidden if a same-list ancestor is also
 * slipped (the user decides at the subtree root); omitted ⇒ cascade off for all.
 */
export function filterCarriedOver(
  tasks: Task[],
  options?: { cascadeEnabledFor?: (listId: string) => boolean },
): Task[] {
  const today = todayIsoKey();
  const overdueIds = new Set(filterOverdue(tasks).map((t) => t.id));
  const slipped = tasks.filter((task) => {
    if (!task.scheduled_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (overdueIds.has(task.id)) return false;
    return task.scheduled_date < today;
  });

  const cascadeFor = options?.cascadeEnabledFor;
  if (!cascadeFor) return slipped;

  const slippedIds = new Set(slipped.map((t) => t.id));
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const hasSlippedAncestor = (task: Task): boolean => {
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
 * Walk all descendants of `rootId` and collect the ones still actionable
 * (`open` / `in_progress`) — for a parent-row verdict that should drag its open
 * children along while leaving settled (completed/cancelled) ones untouched.
 */
export function actionableDescendants(rootId: string, tasks: Task[]): Task[] {
  const out: Task[] = [];
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop() as string;
    for (const t of tasks) {
      if (t.parent_id !== id) continue;
      stack.push(t.id);
      if (t.status === 'open' || t.status === 'in_progress') out.push(t);
    }
  }
  return out;
}

/**
 * Open / in_progress tasks whose `deadline_date` is today and that aren't
 * already pinned to today (`scheduled_date !== today`) — the silent
 * "by"-deadline auto-pin. The scheduled-date check keeps the batch idempotent
 * across re-launches inside the same calendar day.
 */
export function filterDeadlinePinTargets(tasks: Task[]): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.deadline_date !== today) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (task.scheduled_date === today) return false;
    return true;
  });
}

/** The Settings → Tasks day-start-trigger pref: `'app-start'` or an `HH:MM`. */
export type DayStartTrigger = string;

/**
 * Whether a day-start checker should fire now:
 *   - `'app-start'`: fire iff never fired (lastFiredDayKey null).
 *   - `HH:MM`: fire iff not yet fired for `todayKey` AND the local clock has
 *     crossed the threshold. Unparseable / out-of-range ⇒ fire immediately.
 * Pure (clock + the marker are passed in) so it's trivially testable.
 */
export function shouldFireToday(
  trigger: DayStartTrigger,
  lastFiredDayKey: string | null,
  todayKey: string,
  now: Date = new Date(),
): boolean {
  if (trigger === 'app-start') return lastFiredDayKey === null;
  if (lastFiredDayKey === todayKey) return false;
  const m = trigger.match(/^(\d{1,2}):(\d{2})$/);
  if (!m) return true;
  const hours = Number(m[1]);
  const minutes = Number(m[2]);
  if (hours > 23 || minutes > 59) return true;
  return now.getHours() > hours || (now.getHours() === hours && now.getMinutes() >= minutes);
}
