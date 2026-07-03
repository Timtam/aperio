import { isMineOrUnassigned } from './taskAssignment';
import { todayIsoKey } from './taskDay';
import type { Task, TaskUser } from './types';

// Pure day-start-review + deadline-pin selectors + the fire-gate, shared by the
// desktop checkers and the mobile day-start checks. No platform deps (storage /
// timers live in the platform layer). See the desktop dayStartReview.ts /
// deadlinePinTargets.ts / useCurrentDayKey.ts for the original prose.

/**
 * "Overdue" = a `deadline_date` strictly before today AND not already
 * completed/cancelled. `scheduled_date` alone doesn't count — that's a planning
 * hint, not a missed commitment.
 */
export function filterOverdue(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (task.deadline_date >= today) return false;
    // A "project" parent (still has open subtasks) is managed via its subtasks —
    // don't nag about the container; it returns here once they're all settled.
    if (hasActionableDescendants(task.id, tasks)) return false;
    // Don't offer a task owned by a concrete OTHER user — someone else handles it.
    return meFor ? isMineOrUnassigned(task.assignees, meFor(task.list_id)) : true;
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
  options?: {
    cascadeEnabledFor?: (listId: string) => boolean;
    meFor?: (listId: string) => TaskUser | null;
  },
): Task[] {
  const today = todayIsoKey();
  const overdueIds = new Set(filterOverdue(tasks).map((t) => t.id));
  const meFor = options?.meFor;
  const slipped = tasks.filter((task) => {
    if (!task.scheduled_date) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (overdueIds.has(task.id)) return false;
    if (task.scheduled_date >= today) return false;
    // NB: project parents are NOT suppressed here. A term-paper parent has no
    // scheduled_date of its own (only a deadline), so it never reaches this
    // slipped set — its dated SUBTASKS do, and they're what we want surfaced.
    // A parent the user DID schedule onto a work day still flows through the
    // cascade-coupling below (decide-at-the-root), so we leave that intact.
    // Don't offer a task owned by a concrete OTHER user — someone else handles it.
    return meFor ? isMineOrUnassigned(task.assignees, meFor(task.list_id)) : true;
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
 * True when `rootId` still has at least one actionable (`open` / `in_progress`)
 * descendant — i.e. it's a "project" parent whose real work lives in its
 * subtasks. The day-start selectors suppress such a parent (the SUBTASKS are the
 * surfaced, asked-about units, and they keep their own day plan); once every
 * subtask is settled the parent is no longer suppressed and returns to normal
 * review behaviour so its own deadline can be closed out. Early-exits.
 */
export function hasActionableDescendants(rootId: string, tasks: Task[]): boolean {
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop() as string;
    for (const t of tasks) {
      if (t.parent_id !== id) continue;
      if (t.status === 'open' || t.status === 'in_progress') return true;
      // a settled node can still host an actionable grandchild — keep walking.
      stack.push(t.id);
    }
  }
  return false;
}

/**
 * Open / in_progress tasks whose `deadline_date` is today and that aren't
 * already pinned to today (`scheduled_date !== today`) — the silent
 * "by"-deadline auto-pin. The scheduled-date check keeps the batch idempotent
 * across re-launches inside the same calendar day.
 */
export function filterDeadlinePinTargets(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
): Task[] {
  const today = todayIsoKey();
  return tasks.filter((task) => {
    if (!task.deadline_date) return false;
    if (task.deadline_date !== today) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (task.scheduled_date === today) return false;
    // Never pin a project parent to today — its subtasks carry the day plan.
    if (hasActionableDescendants(task.id, tasks)) return false;
    // Don't silently pin a task owned by a concrete OTHER user to my today.
    return meFor ? isMineOrUnassigned(task.assignees, meFor(task.list_id)) : true;
  });
}

// ── Day-start TASK REMINDERS ────────────────────────────────────────────────
// Three reminders surfaced at the day-start trigger (alongside carry-over /
// deadline-pin), each gated by its own Settings toggle. Same structural rules as
// the other selectors: skip settled tasks, suppress "project" parents (their
// open subtasks are the real units), and never remind about a task owned by a
// concrete OTHER user. PURE — `todayIsoKey()` reads the local wall-clock.

/** Whole local calendar days from `fromDayKey` (default: today) until
 *  `task.deadline_date`: 0 = that day, negative = past, null = no deadline.
 *  Drives the countdown WINDOW check + the per-task "in N days" label. Rounds
 *  so a 23/25h DST day doesn't drift. The anchor parameter lets the mobile
 *  scheduler pre-compute FUTURE days' reminder groups for ahead-of-time OS
 *  notifications. */
export function daysUntilDeadline(
  task: Task,
  fromDayKey: string = todayIsoKey(),
): number | null {
  if (!task.deadline_date) return null;
  const [ty, tm, td] = fromDayKey.split('-').map(Number);
  const [dy, dm, dd] = task.deadline_date.split('-').map(Number);
  const todayMs = new Date(ty, tm - 1, td).getTime();
  const deadlineMs = new Date(dy, dm - 1, dd).getTime();
  return Math.round((deadlineMs - todayMs) / 86_400_000);
}

/**
 * Tasks scheduled for TODAY with NO time-of-day (`scheduled_time` null) and
 * still actionable — the "you planned these for today" nudge. A task with a
 * concrete scheduled_time already shows on the calendar's timeline, so it's not
 * part of this untimed reminder.
 */
export function filterUntimedToday(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
  dayKey: string = todayIsoKey(),
): Task[] {
  const today = dayKey;
  return tasks.filter((task) => {
    if (task.scheduled_date !== today) return false;
    if (task.scheduled_time != null) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (hasActionableDescendants(task.id, tasks)) return false;
    return meFor ? isMineOrUnassigned(task.assignees, meFor(task.list_id)) : true;
  });
}

/**
 * Tasks whose `deadline_date` is TODAY and still actionable — "the deadline is
 * here". Unlike `filterDeadlinePinTargets` this does NOT exclude tasks already
 * scheduled to today: the reminder fires regardless of whether the silent
 * deadline-pin also moves it (the pin runs separately, after).
 */
export function filterDeadlineArrived(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
  dayKey: string = todayIsoKey(),
): Task[] {
  const today = dayKey;
  return tasks.filter((task) => {
    if (task.deadline_date !== today) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    if (hasActionableDescendants(task.id, tasks)) return false;
    return meFor ? isMineOrUnassigned(task.assignees, meFor(task.list_id)) : true;
  });
}

/**
 * Tasks whose deadline is APPROACHING — within the countdown WINDOW: the deadline
 * is between 1 and the window-many days away (inclusive), still actionable. So a
 * window of 3 nudges CUMULATIVELY — 3 days before, 2 days before, AND 1 day
 * before (each day the deadline draws closer, the task stays in the set until the
 * deadline day itself, which is `filterDeadlineArrived`, not this).
 *
 * `daysUntil` is the GLOBAL default window (`tasks.deadlineCountdownDays`). A task
 * overrides it via `deadline_reminder_days` (honoured only when finite `>= 1`; a
 * `null`/non-finite/`< 1` override falls back to the global). The per-task
 * remaining days for the label is `daysUntilDeadline(task)`.
 */
export function filterDeadlineCountdown(
  tasks: Task[],
  daysUntil: number,
  meFor?: (listId: string) => TaskUser | null,
  dayKey: string = todayIsoKey(),
): Task[] {
  const globalValid = Number.isFinite(daysUntil) && daysUntil >= 1;
  return tasks.filter((task) => {
    // Per-task override wins only when it's a finite window >= 1; else the global.
    const override = task.deadline_reminder_days;
    const window =
      override != null && Number.isFinite(override) && override >= 1
        ? override
        : globalValid
          ? daysUntil
          : null;
    if (window == null) return false;
    if (task.status === 'completed' || task.status === 'cancelled') return false;
    const days = daysUntilDeadline(task, dayKey);
    // 1..window: skip the deadline day (0 → filterDeadlineArrived) and anything
    // already past or beyond the window.
    if (days == null || days < 1 || days > window) return false;
    if (hasActionableDescendants(task.id, tasks)) return false;
    return meFor ? isMineOrUnassigned(task.assignees, meFor(task.list_id)) : true;
  });
}

/** The four Settings → Tasks reminder knobs (synced). */
export interface ReminderSettings {
  remindUntimedToday: boolean;
  remindDeadlineArrived: boolean;
  remindDeadlineCountdown: boolean;
  deadlineCountdownDays: number;
}

/** The three reminder groups, DE-DUPLICATED by task id. */
export interface ReminderGroups {
  /** Scheduled today, untimed (and not already due-today). */
  untimed: Task[];
  /** Deadline is today. */
  dueToday: Task[];
  /** Deadline exactly `deadlineCountdownDays` out (and not already in another group). */
  countdown: Task[];
}

/**
 * Build the three reminder groups for the day-start fire, each gated by its
 * toggle and DE-DUPLICATED so a task surfaces in exactly ONE group (and is
 * counted once). Priority: due-today > planned-today > countdown — a task the
 * deadline-pin just pinned to today (so it satisfies BOTH `filterDeadlineArrived`
 * and `filterUntimedToday`) reads as "due today", and a task already planned for
 * today isn't also nagged about its future deadline. Both the checker (count +
 * notification + announcement) and the dialog (rendered rows) call this, so the
 * spoken count, the OS notification, and the visible rows always agree.
 */
export function buildReminderGroups(
  tasks: Task[],
  settings: ReminderSettings,
  meFor?: (listId: string) => TaskUser | null,
  dayKey: string = todayIsoKey(),
): ReminderGroups {
  const dueToday = settings.remindDeadlineArrived
    ? filterDeadlineArrived(tasks, meFor, dayKey)
    : [];
  const seen = new Set(dueToday.map((t) => t.id));
  const untimed = (
    settings.remindUntimedToday ? filterUntimedToday(tasks, meFor, dayKey) : []
  ).filter((t) => !seen.has(t.id));
  untimed.forEach((t) => seen.add(t.id));
  const countdown = (
    settings.remindDeadlineCountdown
      ? filterDeadlineCountdown(tasks, settings.deadlineCountdownDays, meFor, dayKey)
      : []
  ).filter((t) => !seen.has(t.id));
  return { untimed, dueToday, countdown };
}

/** Total de-duplicated reminder count across the three groups. */
export function reminderCount(groups: ReminderGroups): number {
  return groups.untimed.length + groups.dueToday.length + groups.countdown.length;
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
