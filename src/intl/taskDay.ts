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

/**
 * Tasks scheduled FOR `day` — the `scheduled_date == day` branch only,
 * without the deadline-window contribution that `filterTasksOnDay`
 * also includes.
 *
 * Used by WeekView so the per-day chip column doesn't double up with
 * the new deadline-bar lane (a task with `deadline_date` overlapping
 * the visible week appears in the bar, NOT as a chip on every day of
 * the window). DayView keeps using `filterTasksOnDay` instead — it
 * has no header lane, so chip-per-day-of-window is still the
 * documented behaviour there.
 *
 * Subtask, status, and completed-opt-in filtering mirrors
 * `filterTasksOnDay`.
 */
export function filterScheduledTasksOnDay(
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
    return task.scheduled_date === dayIsoKey;
  });
}

/** Same shape as `groupTasksByDay` but uses the scheduled-only filter. */
export function groupScheduledTasksByDay(
  tasks: Task[],
  dayKeys: string[],
  isCompletedVisible?: (listId: string) => boolean,
): Map<string, Task[]> {
  const out = new Map<string, Task[]>();
  for (const key of dayKeys) {
    out.set(
      key,
      filterScheduledTasksOnDay(tasks, key, isCompletedVisible),
    );
  }
  return out;
}

/**
 * One row in the deadline-header lane WeekView renders above its day
 * grid (DESIGN.md § 9.4). Each bar represents a task whose deadline
 * window — `[max(today, weekStart), deadline_date]` — overlaps the
 * visible week. The bar spans those day columns, with chevrons when
 * the window extends past the week on either side.
 *
 * Lane-packed greedily: bars sorted by start, longer-first as a
 * tiebreaker, then placed into the lowest lane that doesn't collide
 * with anything already there. Matches `buildAllDayBars`' shape so
 * the CSS treatment can be near-identical.
 */
export interface DeadlineBar {
  task: Task;
  /** 1-based grid column for `gridColumn: start / end+1`. */
  startCol: number;
  endCol: number;
  /** 0-based lane row inside the lane container. */
  lane: number;
  /** Window started before the visible week — render a left chevron. */
  continuesBefore: boolean;
  /** Deadline is after the visible week — render a right chevron. */
  continuesAfter: boolean;
}

/**
 * Build the deadline-header bars for one visible week.
 *
 * `dayKeys` must be the seven ISO day keys (Mon–Sun, or however the
 * week is configured) in display order. `todayIsoKey` decides the
 * window's left edge — past days of the window never render.
 */
export function buildDeadlineBars(
  tasks: Task[],
  dayKeys: string[],
  todayIsoKey: string,
  isCompletedVisible?: (listId: string) => boolean,
): DeadlineBar[] {
  if (dayKeys.length === 0) return [];
  const weekStart = dayKeys[0];
  const weekEnd = dayKeys[dayKeys.length - 1];
  // Left boundary of every bar: whichever is later, today or the week
  // start. When the user navigates to a future week, the bar covers
  // the whole week (today is before weekStart, so the boundary IS
  // weekStart). When today is inside the visible week, the bar starts
  // at today.
  const startBoundary = todayIsoKey > weekStart ? todayIsoKey : weekStart;
  // If "today" is past the visible week's end (the user is looking
  // at a week entirely in the past) — no bars. The deadline-window
  // logic doesn't backfill historical days.
  if (startBoundary > weekEnd) return [];

  const candidates: Array<{
    task: Task;
    startIdx: number;
    endIdx: number;
    continuesBefore: boolean;
    continuesAfter: boolean;
  }> = [];

  for (const task of tasks) {
    if (task.parent_id) continue;
    if (!task.deadline_date) continue;
    if (task.status === 'cancelled') continue;
    if (task.status === 'completed') {
      if (!isCompletedVisible || !isCompletedVisible(task.list_id)) {
        continue;
      }
    }
    // The window ends at deadline_date. If the deadline is before
    // our left boundary (e.g., yesterday), there's no overlap.
    if (task.deadline_date < startBoundary) continue;

    const visStart = startBoundary;
    const visEnd =
      task.deadline_date < weekEnd ? task.deadline_date : weekEnd;
    const startIdx = dayKeys.indexOf(visStart);
    const endIdx = dayKeys.indexOf(visEnd);
    if (startIdx < 0 || endIdx < 0 || startIdx > endIdx) continue;

    candidates.push({
      task,
      startIdx,
      endIdx,
      // "continuesBefore" applies when the user is viewing a future
      // week and the task's window started in a previous week —
      // today is before weekStart. (The other case where the window
      // started earlier — today inside the week, bar starts at today
      // — doesn't add a left chevron because the bar lines up with
      // today's column.)
      continuesBefore: todayIsoKey < weekStart,
      continuesAfter: task.deadline_date > weekEnd,
    });
  }

  // Greedy lane-packing: sort by start, longer first as a tiebreaker
  // (longer bars are harder to fit later). Then for each bar pick
  // the lowest lane whose previous occupant ends before this bar
  // starts.
  candidates.sort(
    (a, b) =>
      a.startIdx - b.startIdx ||
      b.endIdx - b.startIdx - (a.endIdx - a.startIdx),
  );
  const laneEnds: number[] = [];
  return candidates.map((c) => {
    let lane = 0;
    while (lane < laneEnds.length && laneEnds[lane] >= c.startIdx) {
      lane += 1;
    }
    if (lane === laneEnds.length) laneEnds.push(c.endIdx);
    else laneEnds[lane] = c.endIdx;
    return {
      task: c.task,
      startCol: c.startIdx + 1,
      endCol: c.endIdx + 1,
      lane,
      continuesBefore: c.continuesBefore,
      continuesAfter: c.continuesAfter,
    };
  });
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
