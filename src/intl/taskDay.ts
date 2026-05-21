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
 * Completed and cancelled tasks are never returned — they belong in
 * the past, not in upcoming calendar slots. (Filtering them here
 * keeps every consumer from having to remember.)
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
): Task[] {
  return tasks.filter((task) => {
    if (task.status === 'completed' || task.status === 'cancelled') {
      return false;
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
): Map<string, Task[]> {
  const out = new Map<string, Task[]>();
  for (const key of dayKeys) {
    out.set(key, filterTasksOnDay(tasks, key, todayIsoKey));
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
