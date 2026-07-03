import { isMineOrUnassigned } from './taskAssignment';
import { taskOrder } from './taskGrouping';
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
export function filterTasksOnDay(
  tasks: Task[],
  dayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
  meFor?: (listId: string) => TaskUser | null,
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
  return onDay.sort(taskOrder);
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
): Map<string, Task[]> {
  const out = new Map<string, Task[]>();
  for (const key of dayKeys) {
    out.set(key, filterTasksOnDay(tasks, key, isCompletedVisible, meFor));
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
