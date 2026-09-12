// The calendar-day task rules — this surface's door into `cal_core::task_day`.
//
// Which tasks a day shows, in which order, and how the backlog rail cuts its
// weeks: that decision lives in the core now, and both surfaces ask it while
// rendering. What stays here is the shell — building the question from what a
// caller holds, laying the answer (positions) back over the caller's own rows,
// and the two things the core deliberately does not do: read the clock
// (`todayIsoKey`) and resolve a UTC instant into a LOCAL day (the device zone,
// `completionDayKey`), which travels in as `completed_day`.
//
// Pinned by `crates/cal-core/tests/fixtures/taskDay.json`, measured from the
// TypeScript this replaced; `taskDay.contract.test.ts` replays it through this
// door, and the core's own contract test reads the same file.

import { localDateKey } from './dateKey';
import type {
  BacklogWeeks,
  BacklogWeeksInput,
  DayInput,
  DayTask,
  DayTaskRow,
  DeadlineSplit,
  DeadlineSplitInput,
  Task,
  TaskUser,
} from './types';
import type { PriorityScale } from './taskStatus';

/**
 * Local `YYYY-MM-DD` for today.
 *
 * Built from the local wall-clock (getFullYear/getMonth/getDate), NOT a
 * `toISOString().slice(0, 10)` — a UTC slice would roll the day over at the
 * wrong moment and mis-bucket the Upcoming/Deferred gate (DESIGN §9.12) and the
 * "resurfaces on" due text near midnight.
 *
 * The one clock reader of this module, and it stays on this side: the core
 * takes the day as a parameter (`tests/core_contracts.rs`).
 */
export function todayIsoKey(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::task_day`. */
export interface TaskDayRules {
  tasksOnDaysJson(inputJson: string): string;
  backlogWeeksJson(inputJson: string): string;
  splitDeadlinesByWeekJson(inputJson: string): string;
  /** The app's language tag, read when the view asks: the order within a
   *  day collates titles, and the user can change it while the app runs. */
  languageTag(): string;
}

let installedRules: TaskDayRules | null = null;

/** Bind this surface's door into the core. */
export function installTaskDayRules(rules: TaskDayRules): void {
  installedRules = rules;
}

function rules(): TaskDayRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // implementation all over again, and its failure is one a screen-reader
    // user meets head on: a task on one day on the phone and on another on
    // the desktop.
    throw new Error(
      'task day rules used before installTaskDayRules() — the surface must ' +
        'bind its door into cal_core::task_day at startup',
    );
  }
  return installedRules;
}

/** The LOCAL day a task was completed on, or null when it carries no instant.
 *
 *  Local, not UTC, and that is the whole subtlety: `completed_at` is a UTC
 *  instant, so a task finished at 23:30 in a positive offset reads as the NEXT
 *  day if the date is taken off the raw string. The same trap already cost the
 *  recurrence resurface a day.
 *
 *  Resolved HERE, with the zone this surface knows, and handed to the core as
 *  a day key: the core reads no zone. */
function completionDayKey(task: Task): string | null {
  if (!task.completed_at) return null;
  const at = new Date(task.completed_at);
  return Number.isNaN(at.getTime()) ? null : localDateKey(at);
}

/** What the rule reads of a task, plus the one thing only this side knows.
 *
 *  An empty string is sent as `null`: the TypeScript this replaced tested these
 *  fields for truthiness, so `''` meant "none" — no day, no time, no parent —
 *  and the core, which parses dates and times, would reject it instead. No
 *  producer in this repository writes one (every row is serde output of a
 *  `Task`), but a door that turns an empty field into a thrown error inside a
 *  `useMemo` would take the whole view down for a row the old code merely
 *  filed as undated. */
function wireTask(task: Task): DayTask {
  return {
    id: task.id,
    list_id: task.list_id,
    title: task.title,
    status: task.status,
    priority: task.priority,
    scheduled_date: task.scheduled_date || null,
    scheduled_time: task.scheduled_time || null,
    scheduled_end_time: task.scheduled_end_time || null,
    deadline_date: task.deadline_date || null,
    deadline_time: task.deadline_time || null,
    parent_id: task.parent_id || null,
    assignees: task.assignees,
    completed_day: completionDayKey(task),
  };
}

/** One crossing for every day asked. The two callbacks are evaluated once
 *  per list here and travel as data, so no caller had to learn the wire. */
function tasksOnDays(
  tasks: Task[],
  days: string[],
  isCompletedVisible: ((listId: string) => boolean) | undefined,
  meFor: ((listId: string) => TaskUser | null) | undefined,
  scale: PriorityScale,
): Map<string, Task[]> {
  const door = rules();
  const listIds = Array.from(new Set(tasks.map((task) => task.list_id)));
  const input: DayInput = {
    tasks: tasks.map(wireTask),
    days,
    completedVisible: isCompletedVisible ? listIds.filter((id) => isCompletedVisible(id)) : [],
    currentUserByList: meFor ? Object.fromEntries(listIds.map((id) => [id, meFor(id)])) : {},
    scale,
    language: door.languageTag(),
  };
  const answer = JSON.parse(door.tasksOnDaysJson(JSON.stringify(input))) as Record<
    string,
    DayTaskRow[]
  >;
  const out = new Map<string, Task[]>();
  for (const day of days) {
    out.set(
      day,
      (answer[day] ?? []).map((row) => tasks[row.task]),
    );
  }
  return out;
}

/**
 * Tasks that should appear on `day` in a calendar view.
 *
 * A task surfaces on a day for either of two independent reasons:
 *
 *   1. `scheduled_date == day` — the user committed to working on the
 *      task that day ("Geplant für", or the legacy concrete date the
 *      old `deadline_type='on'` migrated into this slot).
 *   2. `deadline_date == day` AND the task has NO `scheduled_date` —
 *      the task is DUE that day and the user hasn't planned a work day,
 *      so the deadline day IS its calendar home (a single point marker,
 *      not a Gantt-style strip across every day until then).
 *
 * A task that has BOTH a scheduled day and a deadline surfaces ONCE — on
 * its scheduled day. A finished task with no planned day belongs to the day
 * it was finished; a finished task the user planned onto a day keeps that
 * day. Subtasks surface only with a date of their own. Cancelled tasks never
 * appear; completed ones only where the caller opts a list in via
 * `isCompletedVisible`. `meFor` gates by ownership: on a shared list a task
 * assigned to a concrete OTHER user is hidden from my calendar (DESIGN §9.7).
 *
 * The decision is the core's (`cal_core::task_day`); this is the door.
 * `dayIsoKey` is the local `YYYY-MM-DD` key, matching `localDateKey()`.
 */
export function filterTasksOnDay(
  tasks: Task[],
  dayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
  meFor?: (listId: string) => TaskUser | null,
  /** The user's priority system — how many bands the ordering has. */
  scale: PriorityScale = 'three',
): Task[] {
  return tasksOnDays(tasks, [dayIsoKey], isCompletedVisible, meFor, scale).get(dayIsoKey) ?? [];
}

/**
 * Bucket helper for week/day views: every day of `dayKeys` at once, one
 * crossing, a Map keyed by ISO day string so the consumer can render each day
 * independently.
 */
export function groupTasksByDay(
  tasks: Task[],
  dayKeys: string[],
  isCompletedVisible?: (listId: string) => boolean,
  meFor?: (listId: string) => TaskUser | null,
  /** The user's priority system — passed straight through to the per-day
   *  ordering (see {@link filterTasksOnDay}). */
  scale: PriorityScale = 'three',
): Map<string, Task[]> {
  return tasksOnDays(tasks, dayKeys, isCompletedVisible, meFor, scale);
}

// ─────────────────────── What a chip carries (per task) ─────────────────────
//
// The core answers these three with every day row (`DayTaskRow`), and the
// views still ask them per chip with (task, day) in hand. Until the views read
// the rows, these stay as the TypeScript twins of `cal_core::task_day::
// {time_on_day, end_time_on_day, is_deadline_chip}`, pinned against them by
// the same fixture: the TypeScript contract test derives its chip facts from
// these, the Rust contract test from the core's fields, and both must match
// the table.

/**
 * True when the task appears on `dayIsoKey` BECAUSE of its deadline,
 * not its scheduled day — i.e. this chip is a "due here" marker rather
 * than a "planned work" chip. A task that is scheduled AND due on the
 * same day is treated as the scheduled chip (schedule wins), so this
 * returns false there. Drives the "fällig bis" aria + the `--by`
 * styling of the deadline marker.
 */
export function isDeadlineChip(task: Task, dayIsoKey: string): boolean {
  return (
    task.deadline_date != null &&
    task.deadline_date === dayIsoKey &&
    task.scheduled_date !== dayIsoKey
  );
}

/**
 * Effective time-of-day at which a task should slot into the timed
 * lane of `dayIsoKey`, or `null` when the task has no specific time
 * on that day: `scheduled_time` when scheduled here, else `deadline_time`
 * when due here. When both apply on the same day the scheduled time wins —
 * it's the "I plan to do it then" commitment, while the deadline_time on the
 * same day is the "must be done by then" cap.
 *
 * Returned shape is the raw `HH:MM[:SS]` string, which sorts
 * lexicographically the same way it sorts numerically.
 */
export function taskTimeOnDay(
  task: Task,
  dayIsoKey: string,
): string | null {
  if (task.scheduled_time && task.scheduled_date === dayIsoKey) {
    return task.scheduled_time;
  }
  if (task.deadline_time && task.deadline_date === dayIsoKey) {
    return task.deadline_time;
  }
  return null;
}

/**
 * The END of a task's planned block on `dayIsoKey`, as `HH:MM[:SS]`, or `null`
 * when it has none there. Only the SCHEDULED slot can carry a block: a
 * deadline is a moment — "by then" — and giving it a length would draw a bar
 * across the hours before something is due.
 */
export function taskEndTimeOnDay(task: Task, dayIsoKey: string): string | null {
  if (
    task.scheduled_end_time &&
    task.scheduled_time &&
    task.scheduled_date === dayIsoKey
  ) {
    return task.scheduled_end_time;
  }
  return null;
}

// ──────────────────────────── Rendering helpers ─────────────────────────────

/**
 * Item types that can appear in a day's time-sorted grid lane. The
 * views render an event chip for `kind: 'event'` and a task chip for
 * `kind: 'task'`, sharing the per-day time column so 09:30 events and
 * 09:45 task deadlines line up the way the user expects.
 */
export type DayGridItem<TEvent, TTask> =
  | { kind: 'event'; event: TEvent; sortKey: number }
  | { kind: 'task'; task: TTask; sortKey: number };

/**
 * Merge events and timed tasks into a single chronologically sorted
 * list for one day. Untimed tasks (those for which `taskTimeOnDay`
 * returned `null`) are returned in a second array so the caller can
 * render them in the existing untimed lane below the grid.
 *
 * `eventTime(event)` returns the event's start as epoch-ms. Composing a task's
 * `HH:MM` on this day into the same scale goes through the local `Date`, so a
 * daylight-saving jump still lands at the user's perceived local time — which
 * is why this is rendering, on this side, and not a rule in the core.
 */
export function mergeDayItems<TEvent, TTask extends Task>(
  events: TEvent[],
  tasks: TTask[],
  dayIsoKey: string,
  eventTime: (e: TEvent) => number,
): { timed: DayGridItem<TEvent, TTask>[]; untimed: TTask[] } {
  const timed: DayGridItem<TEvent, TTask>[] = events.map((event) => ({
    kind: 'event' as const,
    event,
    sortKey: eventTime(event),
  }));
  const untimed: TTask[] = [];
  for (const task of tasks) {
    const time = taskTimeOnDay(task, dayIsoKey);
    if (time === null) {
      untimed.push(task);
      continue;
    }
    const [hh, mm, ss] = time.split(':').map((n) => Number(n));
    const [y, mo, d] = dayIsoKey.split('-').map((n) => Number(n));
    const ms = new Date(y, mo - 1, d, hh ?? 0, mm ?? 0, ss ?? 0).getTime();
    timed.push({ kind: 'task', task, sortKey: ms });
  }
  timed.sort((a, b) => a.sortKey - b.sortKey);
  return { timed, untimed };
}

// ───────────────────────────── The backlog rail ─────────────────────────────

/**
 * The current and following CALENDAR week, as inclusive day-key bounds —
 * `cal_core::task_day::backlog_weeks`. Calendar weeks, not rolling windows:
 * "this week" ends on the week's last day however near that is. `weekStartsOn`
 * is the user's own setting (0 = Sunday … 6 = Saturday).
 */
export function backlogWeeks(todayKey: string, weekStartsOn: number): BacklogWeeks {
  // The setting is 0..6 on every surface that has one; the old arithmetic
  // happened to tolerate anything, so keep that at the door.
  const input: BacklogWeeksInput = { today: todayKey, weekStartsOn: ((weekStartsOn % 7) + 7) % 7 };
  return JSON.parse(rules().backlogWeeksJson(JSON.stringify(input))) as BacklogWeeks;
}

/**
 * Split deadline-carrying tasks into this week, next week and everything after
 * — `cal_core::task_day::split_deadlines_by_week`. Every bucket keeps the
 * order it was handed; a deadline that has already passed goes with THIS week.
 */
export function splitDeadlinesByWeek<T extends { deadline_date?: string | null }>(
  tasks: readonly T[],
  weeks: BacklogWeeks,
): { thisWeek: T[]; nextWeek: T[]; later: T[] } {
  const input: DeadlineSplitInput = {
    deadlines: tasks.map((task) => task.deadline_date || null),
    weeks,
  };
  const split = JSON.parse(rules().splitDeadlinesByWeekJson(JSON.stringify(input))) as DeadlineSplit;
  const pick = (at: number[]): T[] => at.map((i) => tasks[i]);
  return { thisWeek: pick(split.thisWeek), nextWeek: pick(split.nextWeek), later: pick(split.later) };
}
