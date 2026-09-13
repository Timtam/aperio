// The day-start rules — this surface's door into `cal_core::day_start`.
//
// Which tasks are overdue, which slipped, which get pinned to today, which are
// reminded and in which group, how many days remain to a deadline, what "move
// to today" does to a task's dates, and whether a day-start checker fires now:
// that decision lives in the core now, and both surfaces ask it every morning —
// the desktop checkers and review dialog, the mobile checks and modal, and the
// mobile scheduler that asks about future days ahead of time. What stays here
// is the shell: the clock (today's key and the local time — the core reads
// none), building the question from what a caller holds (a user is its id; a
// list's identity and coupling are asked once per list), and laying the
// answer — positions — back over the caller's own task objects. No platform
// deps: storage and timers live in the platform layer.
//
// Pinned by `crates/cal-core/tests/fixtures/dayStart.json`, measured from the
// TypeScript this replaced; `dayStart.contract.test.ts` replays it through
// this door, and the core's own contract test reads the same file.

import { todayIsoKey } from './taskDay';
import type {
  DayStartIdentity,
  DayStartMoved,
  DayStartQuestion,
  DayStartReminderGroups,
  DayStartTask,
  Task,
  TaskUser,
} from './types';

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::day_start`: one question, one answer. */
export interface DayStartRules {
  dayStartJson(inputJson: string): string;
}

let installedRules: DayStartRules | null = null;

/** Bind this surface's door into the core. */
export function installDayStartRules(rules: DayStartRules): void {
  installedRules = rules;
}

function ask<T>(question: DayStartQuestion): T {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the copy all over
    // again, and its failure is one nobody reports: a morning that offers
    // different tasks on the phone and on the desktop.
    throw new Error(
      'day-start rules used before installDayStartRules() — the surface must ' +
        'bind its door into cal_core::day_start at startup',
    );
  }
  return JSON.parse(installedRules.dayStartJson(JSON.stringify(question))) as T;
}

/** What the rules read of each task. A user is its id. */
function wire(tasks: Task[]): DayStartTask[] {
  return tasks.map((t) => ({
    id: t.id,
    list_id: t.list_id,
    status: t.status,
    parent_id: t.parent_id ?? null,
    scheduled_date: t.scheduled_date ?? null,
    scheduled_time: t.scheduled_time ?? null,
    deadline_date: t.deadline_date ?? null,
    deadline_reminder_days: t.deadline_reminder_days ?? null,
    assignees: t.assignees.map((a) => a.id),
  }));
}

/** The lists the tasks live in, each once, in first-seen order. */
function listsOf(tasks: Task[]): string[] {
  return [...new Set(tasks.map((t) => t.list_id))];
}

/** The account's own id per list; no `meFor`, no identities — nothing filtered. */
function identities(
  tasks: Task[],
  meFor: ((listId: string) => TaskUser | null) | undefined,
): DayStartIdentity[] {
  if (!meFor) return [];
  return listsOf(tasks).map((list_id) => ({ list_id, me: meFor(list_id)?.id ?? null }));
}

/** A window as the wire can carry it: `NaN` and the infinities are no window. */
function windowOf(days: number): number | null {
  return Number.isFinite(days) ? days : null;
}

function pick(tasks: Task[], positions: number[]): Task[] {
  return positions.map((i) => tasks[i]);
}

// ─────────────────────────────── The rules ──────────────────────────────────

/**
 * "Overdue" = a `deadline_date` strictly before the day AND not already
 * completed/cancelled. `scheduled_date` alone doesn't count — that's a planning
 * hint, not a missed commitment. A "project" parent (still has open subtasks)
 * is managed via its subtasks and returns here once they're all settled; a task
 * owned by a concrete OTHER user is someone else's to handle.
 */
export function filterOverdue(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
  // The anchor day, like the reminder selectors carry it: the mobile
  // scheduler pre-computes FUTURE days' counts for ahead-of-time OS
  // notifications. Defaults to today — the live checkers pass nothing.
  dayKey: string = todayIsoKey(),
): Task[] {
  return pick(
    tasks,
    ask<number[]>({
      rule: 'overdue',
      tasks: wire(tasks),
      today: dayKey,
      identities: identities(tasks, meFor),
    }),
  );
}

/**
 * Tasks with a `scheduled_date` strictly before the day, still actionable
 * (`open` / `in_progress`), and NOT already in the overdue list (the deadline
 * is the bigger lever, shown in that section — and because that section's own
 * "today" answer, {@link movedToToday}, carries a lapsed plan along, the task
 * is fully settled there rather than half-answered in two places). When
 * `cascadeEnabledFor` is given, a slipped task in a cascading list is hidden if
 * an ancestor is also slipped (the user decides at the subtree root); only the
 * task's own list is asked. Omitted ⇒ cascade off for all.
 */
export function filterCarriedOver(
  tasks: Task[],
  options?: {
    cascadeEnabledFor?: (listId: string) => boolean;
    meFor?: (listId: string) => TaskUser | null;
  },
  /** Anchor day — see {@link filterOverdue}. */
  dayKey: string = todayIsoKey(),
): Task[] {
  const cascadeFor = options?.cascadeEnabledFor;
  return pick(
    tasks,
    ask<number[]>({
      rule: 'carried_over',
      tasks: wire(tasks),
      today: dayKey,
      identities: identities(tasks, options?.meFor),
      coupled_lists: cascadeFor ? listsOf(tasks).filter((listId) => cascadeFor(listId)) : [],
    }),
  );
}

/**
 * Walk all descendants of `rootId` and collect the ones still actionable
 * (`open` / `in_progress`) — for a parent-row verdict that should drag its open
 * children along while leaving settled (completed/cancelled) ones untouched.
 */
export function actionableDescendants(rootId: string, tasks: Task[]): Task[] {
  return pick(
    tasks,
    ask<number[]>({ rule: 'actionable_descendants', tasks: wire(tasks), root_id: rootId }),
  );
}

/**
 * The task as it stands after "move the lapsed deadline to today".
 *
 * Shared because the rule is the interesting part and both platforms have to
 * obey the same one — the desktop dialog and the mobile modal each own their
 * own write, and a rule that lives in two places is a rule that ends up
 * applied in one.
 *
 * The TIME stays. A deadline of 14:00 that slipped is still a 14:00 deadline;
 * clearing it would silently turn a commitment into "sometime today", which is
 * not what the user wrote and not what they asked for.
 *
 * A PLAN that also lapsed comes along: `filterCarriedOver` deliberately skips
 * anything the deadline section already shows, so a task whose deadline AND
 * plan lapsed never reaches the carry-over section, and answering "today" must
 * not leave the plan stranded in the past. A plan that is already today, or
 * still ahead, is the user's own arrangement and stays untouched.
 */
export function movedToToday<
  T extends {
    deadline_date?: string | null;
    scheduled_date?: string | null;
  },
>(task: T): T {
  const moved = ask<DayStartMoved>({
    rule: 'moved_to_today',
    today: todayIsoKey(),
    scheduled_date: task.scheduled_date ?? null,
  });
  return {
    ...task,
    deadline_date: moved.deadline_date,
    ...(moved.scheduled_date === undefined ? {} : { scheduled_date: moved.scheduled_date }),
  };
}

/**
 * True when `rootId` still has at least one actionable (`open` / `in_progress`)
 * descendant — i.e. it's a "project" parent whose real work lives in its
 * subtasks. The day-start selectors suppress such a parent (the SUBTASKS are the
 * surfaced, asked-about units, and they keep their own day plan); once every
 * subtask is settled the parent returns to normal review behaviour so its own
 * deadline can be closed out.
 */
export function hasActionableDescendants(rootId: string, tasks: Task[]): boolean {
  return ask<boolean>({
    rule: 'has_actionable_descendants',
    tasks: wire(tasks),
    root_id: rootId,
  });
}

/**
 * Open / in_progress tasks whose `deadline_date` is today and that aren't
 * already pinned to today (`scheduled_date !== today`) — the silent
 * "by"-deadline auto-pin. The scheduled-date check keeps the batch idempotent
 * across re-launches inside the same calendar day. Never a project parent (its
 * subtasks carry the day plan), never a task owned by a concrete OTHER user.
 */
export function filterDeadlinePinTargets(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
): Task[] {
  return pick(
    tasks,
    ask<number[]>({
      rule: 'deadline_pin_targets',
      tasks: wire(tasks),
      today: todayIsoKey(),
      identities: identities(tasks, meFor),
    }),
  );
}

// ── Day-start TASK REMINDERS ────────────────────────────────────────────────
// Three reminders surfaced at the day-start trigger (alongside carry-over /
// deadline-pin), each gated by its own Settings toggle. Same structural rules as
// the other selectors: skip settled tasks, suppress "project" parents (their
// open subtasks are the real units), and never remind about a task owned by a
// concrete OTHER user.

/** Whole calendar days from `fromDayKey` (default: today) until
 *  `task.deadline_date`: 0 = that day, negative = past, null = no deadline, or
 *  a key that is not a `YYYY-MM-DD` day — refused rather than answered with a
 *  confident wrong number. Drives the countdown WINDOW check + the per-task
 *  "in N days" label. The anchor parameter lets the mobile scheduler pre-compute
 *  FUTURE days' reminder groups for ahead-of-time OS notifications. */
export function daysUntilDeadline(
  task: Task,
  fromDayKey: string = todayIsoKey(),
): number | null {
  return ask<number | null>({
    rule: 'days_until_deadline',
    deadline_date: task.deadline_date ?? null,
    from: fromDayKey,
  });
}

/**
 * Tasks scheduled for the day with NO time-of-day (`scheduled_time` null) and
 * still actionable — the "you planned these for today" nudge. A task with a
 * concrete scheduled_time already shows on the calendar's timeline, so it's not
 * part of this untimed reminder.
 */
export function filterUntimedToday(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
  dayKey: string = todayIsoKey(),
): Task[] {
  return pick(
    tasks,
    ask<number[]>({
      rule: 'untimed_today',
      tasks: wire(tasks),
      today: dayKey,
      identities: identities(tasks, meFor),
    }),
  );
}

/**
 * Tasks whose `deadline_date` is the day and still actionable — "the deadline
 * is here". Unlike `filterDeadlinePinTargets` this does NOT exclude tasks
 * already scheduled to today: the reminder fires regardless of whether the
 * silent deadline-pin also moves it (the pin runs separately, after).
 */
export function filterDeadlineArrived(
  tasks: Task[],
  meFor?: (listId: string) => TaskUser | null,
  dayKey: string = todayIsoKey(),
): Task[] {
  return pick(
    tasks,
    ask<number[]>({
      rule: 'deadline_arrived',
      tasks: wire(tasks),
      today: dayKey,
      identities: identities(tasks, meFor),
    }),
  );
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
  return pick(
    tasks,
    ask<number[]>({
      rule: 'deadline_countdown',
      tasks: wire(tasks),
      today: dayKey,
      identities: identities(tasks, meFor),
      days_until: windowOf(daysUntil),
    }),
  );
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
  /** Deadline within the countdown window (and not already in another group). */
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
  const groups = ask<DayStartReminderGroups>({
    rule: 'reminder_groups',
    tasks: wire(tasks),
    today: dayKey,
    identities: identities(tasks, meFor),
    settings: {
      remind_untimed_today: settings.remindUntimedToday,
      remind_deadline_arrived: settings.remindDeadlineArrived,
      remind_deadline_countdown: settings.remindDeadlineCountdown,
      deadline_countdown_days: windowOf(settings.deadlineCountdownDays),
    },
  });
  return {
    untimed: pick(tasks, groups.untimed),
    dueToday: pick(tasks, groups.due_today),
    countdown: pick(tasks, groups.countdown),
  };
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
 * The clock and the marker are passed in; the shell reads only the local hour
 * and minute off `now`.
 */
export function shouldFireToday(
  trigger: DayStartTrigger,
  lastFiredDayKey: string | null,
  todayKey: string,
  now: Date = new Date(),
): boolean {
  return ask<boolean>({
    rule: 'should_fire',
    trigger,
    last_fired: lastFiredDayKey,
    today: todayKey,
    now_hour: now.getHours(),
    now_minute: now.getMinutes(),
  });
}
