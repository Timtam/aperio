import { localDateKey } from './dateKey';
import { isMineOrUnassigned } from './taskAssignment';
import { taskOrder } from './taskGrouping';
import type { PriorityScale } from './taskStatus';
import type { Task, TaskUser } from './types';

// Pure date + calendar-bucketing helpers shared by the desktop and mobile
// frontends. The Day/Week calendar views surface tasks alongside events; these
// helpers decide which tasks land on which day, at which time, and how an
// event+task lane sorts. Kept platform-agnostic (no date-fns, no React) so both
// `src/` and `mobile/` import the one source of truth.

/**
 * Local `YYYY-MM-DD` for today.
 *
 * Built from the local wall-clock (getFullYear/getMonth/getDate), NOT a
 * `toISOString().slice(0, 10)` — a UTC slice would roll the day over at the
 * wrong moment and mis-bucket the Upcoming/Deferred gate (DESIGN §9.12) and the
 * "resurfaces on" due text near midnight.
 */
export function todayIsoKey(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
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
 * its scheduled day. It does NOT also appear on the deadline day: the
 * plan is its home, and the chip announces its deadline there ("fällig
 * bis …"). Showing it on the deadline day too read as a duplicate / a
 * second task. (A scheduled task that slips past its day is still surfaced
 * by the day-start review + carry-over, so suppressing the deadline-day
 * marker doesn't lose it.)
 *
 * Subtasks (`parent_id` set) are hidden UNLESS they carry their OWN
 * `scheduled_date` or `deadline_date` — then the planned/due subtask
 * surfaces as its own chip on that day (the chip names its parent so it
 * reads in context). An undated subtask still travels with its parent and
 * stays hidden. Cancelled tasks never appear. Completed tasks are
 * hidden by default — they're done — but the caller can opt back in per
 * task-list via `isCompletedVisible`, the sidebar setting "Erledigte
 * Aufgaben in der Kalenderansicht anzeigen". When the callback is
 * omitted (tests, one-off callers) the historical "always hide" applies.
 *
 * `dayIsoKey` is the local `YYYY-MM-DD` key, matching `localDateKey()`
 * in `dateKey.ts`, so the Week/Day callers share one bucket loop.
 *
 * `meFor` (optional) gates by ownership: on a shared list a task assigned to a
 * concrete OTHER user is theirs to handle, so it's hidden from MY calendar
 * (mine + unassigned stay). A list with no known identity (`meFor` → null) keeps
 * everything; omitting `meFor` keeps the historical "show all". Mirrors the
 * day-start review's ownership filter (DESIGN §9.7).
 */
/** The LOCAL day a task was completed on, or null when it carries no instant.
 *
 *  Local, not UTC, and that is the whole subtlety: `completed_at` is a UTC
 *  instant, so a task finished at 23:30 in a positive offset reads as the NEXT
 *  day if the date is taken off the raw string. The same trap already cost the
 *  recurrence resurface a day. */
function completionDayKey(task: Task): string | null {
  if (!task.completed_at) return null;
  const at = new Date(task.completed_at);
  return Number.isNaN(at.getTime()) ? null : localDateKey(at);
}

export function filterTasksOnDay(
  tasks: Task[],
  dayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
  meFor?: (listId: string) => TaskUser | null,
  /** The user's priority system — how many bands the ordering below has.
   *  Defaults to the three-level original (see {@link taskOrder}). */
  scale: PriorityScale = 'three',
): Task[] {
  const onDay = tasks.filter((task) => {
    // A subtask surfaces only when it carries its own date — an undated subtask
    // travels with its parent and stays hidden; a scheduled/deadline-bearing one
    // becomes its own day chip (labelled as a subtask of its parent).
    if (task.parent_id && !task.scheduled_date && !task.deadline_date) {
      return false;
    }
    if (task.status === 'cancelled') return false;
    if (task.status === 'completed') {
      if (!isCompletedVisible || !isCompletedVisible(task.list_id)) {
        return false;
      }
    }
    if (meFor && !isMineOrUnassigned(task.assignees, meFor(task.list_id))) {
      return false;
    }
    // A FINISHED task belongs to the day it was finished, not to the day it was
    // once due. Anything else makes the calendar disagree with what happened: a
    // task due Thursday and done on Wednesday left Wednesday looking empty and
    // put a tick on Thursday for work that was already over — and read, on the
    // day itself, as something still to do.
    //
    // Falls through when the completion instant is missing (an adapter that
    // does not record one), because the planned day is a worse answer than the
    // right one but a much better answer than none.
    if (task.status === 'completed') {
      const finished = completionDayKey(task);
      if (finished != null) return finished === dayIsoKey;
    }
    if (task.scheduled_date === dayIsoKey) return true;
    // A deadline surfaces as its own day marker ONLY for a task with no
    // scheduled day. A scheduled task lives on its scheduled day and
    // announces its deadline there, so it does not also appear on the
    // deadline day (that duplicate read as a second task / a recurrence).
    if (!task.scheduled_date && task.deadline_date === dayIsoKey) return true;
    return false;
  });
  // Same order as the task list (priority band, then natural A→Z title) so a
  // day's planned work reads identically on every surface. Timed tasks are
  // re-sorted chronologically by `mergeDayItems`; the untimed lane and the
  // month cells keep this order. (`filter` returned a fresh array, so the
  // in-place sort can't reorder the caller's snapshot.)
  return onDay.sort((a, b) => taskOrder(a, b, scale));
}

/**
 * Bucket helper for week/day views. Returns a Map keyed by ISO day
 * string so the consumer can render each day independently.
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
  const out = new Map<string, Task[]>();
  for (const key of dayKeys) {
    out.set(key, filterTasksOnDay(tasks, key, isCompletedVisible, meFor, scale));
  }
  return out;
}

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
 * on that day.
 *
 * Two distinct slots can contribute a time:
 *
 *   - `scheduled_time` when `scheduled_date === dayIsoKey`. The user
 *     planned to work on the task at that minute on that day.
 *   - `deadline_time` when `deadline_date === dayIsoKey`. The user
 *     marked the deadline with a specific time-of-day.
 *
 * When both apply on the same day (the rare Plan + Soft-Deadline
 * configuration that happens to collide on one day) the scheduled
 * time wins — it's the "I plan to do it then" commitment, while the
 * deadline_time on the same day is the "must be done by then" cap.
 * Showing the schedule wins because it's the more action-oriented
 * marker for that day.
 *
 * Tasks scheduled to a day without a time and bare deadline-day tasks
 * without a `deadline_time` return `null` — there's no minute we can
 * honestly point at, so they keep their place in the untimed lane
 * below the day's grid items.
 *
 * Returned shape is the raw `HH:MM[:SS]` string, which sorts
 * lexicographically the same way it sorts numerically — cheap and
 * matches how event start times are compared elsewhere.
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
 * `eventTime(event)` returns the event's start as epoch-ms — keeping
 * the helper generic over the project's event type means we can unit-
 * test it without importing the full CalendarEvent shape.
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
    // Compose an epoch-ms sort key from the day key + the
    // `HH:MM[:SS]` time string so the comparison is uniform with the
    // event start times. We parse via `Date` so a daylight-saving
    // jump still lands at the user's perceived local time.
    const [hh, mm, ss] = time.split(':').map((n) => Number(n));
    const [y, mo, d] = dayIsoKey.split('-').map((n) => Number(n));
    const ms = new Date(y, mo - 1, d, hh ?? 0, mm ?? 0, ss ?? 0).getTime();
    timed.push({ kind: 'task', task, sortKey: ms });
  }
  timed.sort((a, b) => a.sortKey - b.sortKey);
  return { timed, untimed };
}

/** The two week windows the backlog rail splits its deadlines into. */
export interface BacklogWeeks {
  /** First day of the week `todayKey` falls in, as a `YYYY-MM-DD` key. */
  thisWeekStart: string;
  /** Last day of that week, inclusive. */
  thisWeekEnd: string;
  nextWeekStart: string;
  /** Last day of the following week, inclusive. */
  nextWeekEnd: string;
}

/**
 * The current and following CALENDAR week, as inclusive day-key bounds.
 *
 * Calendar weeks, not rolling windows: "this week" ends on the week's last day
 * however near that is, so a Friday deadline stops being "this week" the moment
 * the week turns — which is what a plan for the week means. Seven days from now
 * would keep sliding and never tell the user where a week ends.
 *
 * `weekStartsOn` is the user's own setting (0 = Sunday … 6 = Saturday), so a
 * Sunday week runs Sunday–Saturday and a Monday week Monday–Sunday.
 *
 * All arithmetic is on LOCAL dates. A deadline is a day the user wrote down,
 * and reading it in UTC puts everyone west of Greenwich a day out.
 */
export function backlogWeeks(todayKey: string, weekStartsOn: number): BacklogWeeks {
  const [y, m, d] = todayKey.split('-').map(Number);
  const today = new Date(y, m - 1, d);
  const start = new Date(today);
  // How far back the week's first day lies. The +7 keeps the result positive
  // for every combination of weekday and setting.
  start.setDate(today.getDate() - ((today.getDay() - weekStartsOn + 7) % 7));
  const dayAfter = (from: Date, days: number) => {
    const out = new Date(from);
    out.setDate(from.getDate() + days);
    return out;
  };
  return {
    thisWeekStart: localDateKey(start),
    thisWeekEnd: localDateKey(dayAfter(start, 6)),
    nextWeekStart: localDateKey(dayAfter(start, 7)),
    nextWeekEnd: localDateKey(dayAfter(start, 13)),
  };
}

/**
 * Split deadline-carrying tasks into this week, next week and everything after.
 *
 * The input keeps whatever order it arrives in — the rail sorts by date, then
 * priority, then creation, and every bucket preserves that.
 *
 * A deadline that has already passed goes in with THIS week rather than into
 * the tail: it is the most urgent thing the rail holds, the date sort puts it
 * at the very top of the first section, and burying last Tuesday's deadline
 * below everything else would be the one placement that helps nobody.
 */
export function splitDeadlinesByWeek<T extends { deadline_date?: string | null }>(
  tasks: readonly T[],
  weeks: BacklogWeeks,
): { thisWeek: T[]; nextWeek: T[]; later: T[] } {
  const thisWeek: T[] = [];
  const nextWeek: T[] = [];
  const later: T[] = [];
  for (const task of tasks) {
    const due = task.deadline_date;
    if (!due) continue;
    if (due <= weeks.thisWeekEnd) thisWeek.push(task);
    else if (due <= weeks.nextWeekEnd) nextWeek.push(task);
    else later.push(task);
  }
  return { thisWeek, nextWeek, later };
}
