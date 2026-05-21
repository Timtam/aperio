import type { Task } from '../api/types';

/**
 * Tasks that should appear on `day` in a calendar view.
 *
 * A task is "on" a day when any of three conditions is true:
 *
 *   1. `scheduled_date == day`  — the user explicitly planned this
 *      task for that day (Backlog → planned).
 *   2. `deadline_type == 'on'` and `deadline_date == day` — the
 *      user committed to finishing this task on that specific day.
 *   3. `deadline_type == 'by'` and the day falls inside the
 *      `[today, deadline_date]` window — the task is in its "still
 *      possible to finish in time" stretch, so it surfaces on every
 *      day inside that stretch (the spec's "an jedem Tag bis zur
 *      Deadline sichtbar"). The window starts at today, not at the
 *      task's creation date, so the calendar isn't cluttered with
 *      past stretches of long-running By-tasks.
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
      task.deadline_type === 'on' &&
      task.deadline_date === dayIsoKey
    ) {
      return true;
    }
    if (
      task.deadline_type === 'by' &&
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
 * A task only carries a meaningful time on its *own* deadline day
 * (`deadline_date === dayIsoKey`) and only when the user actually
 * picked a time (`deadline_time != null`). Tasks scheduled to a day
 * without a time, By-tasks spanning a window, and the bare "today"
 * slot of a long-running By-task all return `null` — there's no
 * minute we can honestly point at, so they keep their place in the
 * untimed lane below the day's grid items.
 *
 * Returned shape is the raw `HH:MM[:SS]` string from `deadline_time`,
 * which sorts lexicographically the same way it sorts numerically —
 * cheap and matches how event start times are compared elsewhere.
 */
export function taskTimeOnDay(
  task: Task,
  dayIsoKey: string,
): string | null {
  if (
    task.deadline_time &&
    task.deadline_date === dayIsoKey
  ) {
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
