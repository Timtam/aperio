import type { Task } from '../api/types';

/**
 * Tasks that should appear on `day` in a calendar view.
 *
 * A task surfaces on a day for either of two independent reasons:
 *
 *   1. `scheduled_date == day` — the user committed to working on the
 *      task that day ("Geplant für", or the legacy concrete date the
 *      old `deadline_type='on'` migrated into this slot).
 *   2. `deadline_date == day` — the task is DUE that day. The deadline
 *      shows as a single point marker on the deadline day, not as a
 *      span across every day until then. (That Gantt-style strip grew
 *      unbounded for far-future deadlines and cluttered the planner;
 *      mainstream task managers show a deadline as a point on its day.)
 *
 * A task with both fields set surfaces twice — a "work" chip on its
 * scheduled day and a "due" marker on its deadline day. When both fall
 * on the same day it appears once: see {@link isDeadlineChip}, where the
 * scheduled chip wins (matching `taskTimeOnDay`'s "schedule wins" rule).
 *
 * Subtasks (`parent_id` set) are unconditionally hidden — they're scoped
 * to their parent and managed via TaskDialog / TaskView, never as
 * standalone chips. Cancelled tasks never appear. Completed tasks are
 * hidden by default — they're done — but the caller can opt back in per
 * task-list via `isCompletedVisible`, the sidebar setting "Erledigte
 * Aufgaben in der Kalenderansicht anzeigen". When the callback is
 * omitted (tests, one-off callers) the historical "always hide" applies.
 *
 * `dayIsoKey` is the local `YYYY-MM-DD` key, matching `localDateKey()`
 * in `intl/dateKey.ts`, so the Week/Day callers share one bucket loop.
 */
export function filterTasksOnDay(
  tasks: Task[],
  dayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
): Task[] {
  return tasks.filter((task) => {
    if (task.parent_id) return false;
    if (task.status === 'cancelled') return false;
    if (task.status === 'completed') {
      if (!isCompletedVisible || !isCompletedVisible(task.list_id)) {
        return false;
      }
    }
    if (task.scheduled_date === dayIsoKey) return true;
    if (task.deadline_date === dayIsoKey) return true;
    return false;
  });
}

/**
 * Bucket helper for week/day views. Returns a Map keyed by ISO day
 * string so the consumer can render each day independently.
 */
export function groupTasksByDay(
  tasks: Task[],
  dayKeys: string[],
  isCompletedVisible?: (listId: string) => boolean,
): Map<string, Task[]> {
  const out = new Map<string, Task[]>();
  for (const key of dayKeys) {
    out.set(key, filterTasksOnDay(tasks, key, isCompletedVisible));
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

/** Local `YYYY-MM-DD` for today — now lives in `@aperio/shared` (reused by the
 *  mobile app); re-exported here so existing `../intl/taskDay` imports are
 *  unchanged. */
export { todayIsoKey } from '@aperio/shared';

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
