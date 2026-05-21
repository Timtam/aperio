import type { Task } from '../api/types';

/**
 * Tasks that should appear on `day` in a calendar view.
 *
 * After migration 0006 a task carries two independent slots, either
 * of which can put the row on a given day:
 *
 *   1. `scheduled_date == day` — the user committed to working on the
 *      task on that day, either as a planned slot ("Geplant für") or
 *      as the legacy "concrete deadline" (the old `deadline_type='on'`
 *      moved into this field by the migration).
 *   2. `deadline_date != null` and the day falls inside the
 *      `[today, deadline_date]` window — the task is in its
 *      "still possible to finish in time" stretch, so it surfaces on
 *      every day inside that stretch (the spec's "an jedem Tag bis
 *      zur Deadline sichtbar"). The window starts at today so the
 *      calendar isn't cluttered with past stretches of long-running
 *      By-tasks. The deadline-day itself is the last day of the
 *      window. (The old `deadline_type='by'` flag is gone; every
 *      `deadline_date` now carries this semantics.)
 *
 * A task with both fields set surfaces on its planned day plus every
 * day in the deadline window — the planned day is typically already
 * inside that window so the practical effect is "shown on the
 * intended-work day and as a reminder every day until the deadline".
 *
 * Subtasks (tasks with `parent_id` set) are unconditionally hidden
 * — they're scoped to their parent and managed via TaskDialog /
 * TaskView, never as standalone chips on the calendar.
 *
 * Cancelled tasks are never returned. Completed tasks are hidden by
 * default — they're done, the user has already moved on — but the
 * caller can opt back in per task-list via `isCompletedVisible`, the
 * sidebar context-menu setting "Erledigte Aufgaben in der
 * Kalenderansicht anzeigen". When the callback is omitted (tests,
 * one-off callers) the historical "always hide" behaviour applies.
 *
 * The `dayIsoKey` argument is the local `YYYY-MM-DD` key, matching
 * `localDateKey()` in `intl/dateKey.ts` — that way the WeekView /
 * DayView callers can share a single bucket-building loop with
 * existing event-grouping helpers.
 */
export function filterTasksOnDay(
  tasks: Task[],
  dayIsoKey: string,
  todayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
): Task[] {
  return tasks.filter((task) => {
    // Subtasks are scoped to their parent: they never surface as
    // their own chip on a calendar surface, even when they carry
    // their own scheduled_date or deadline_date. Manage them via
    // the parent's TaskDialog or under the parent in TaskView. The
    // calendar is for top-level items only.
    if (task.parent_id) return false;
    if (task.status === 'cancelled') return false;
    if (task.status === 'completed') {
      // Completed rows surface only when the user opted-in for this
      // specific list. Default (no callback / callback returns false)
      // matches the original behaviour: completed tasks are hidden
      // from calendar views and live on in TaskView only.
      if (!isCompletedVisible || !isCompletedVisible(task.list_id)) {
        return false;
      }
    }
    if (task.scheduled_date === dayIsoKey) return true;
    if (
      task.deadline_date &&
      dayIsoKey >= todayIsoKey &&
      dayIsoKey <= task.deadline_date
    ) {
      return true;
    }
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
  todayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
): Map<string, Task[]> {
  const out = new Map<string, Task[]>();
  for (const key of dayKeys) {
    out.set(
      key,
      filterTasksOnDay(tasks, key, todayIsoKey, isCompletedVisible),
    );
  }
  return out;
}

/** Local `YYYY-MM-DD` for today — convenience for the caller. */
export function todayIsoKey(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
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
 * Tasks scheduled to a day without a time, By-tasks spanning a
 * window, and the bare "today" slot of a long-running By-task all
 * return `null` — there's no minute we can honestly point at, so
 * they keep their place in the untimed lane below the day's grid
 * items.
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
