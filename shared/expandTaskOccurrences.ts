import { fromBackend, type TaskRecurrenceValue } from './taskRecurrence';
import type { Task } from './types';

/**
 * Expand recurring SCHEDULED tasks into virtual per-day occurrences across a
 * window, so a recurring dated task shows on EVERY planned day in the calendar
 * views (like a recurring event), not only its single current `scheduled_date`.
 *
 * Only DETERMINISTIC, DATED, SCHEDULE-placement recurrences can be pre-computed:
 *   - `anchor = from_date` (the dates follow from the task's own date; a
 *     `from_completion` rule's future dates depend on when each turn is checked
 *     off, so they're unknowable ahead of time), AND
 *   - `placement = schedule` (a `backlog` rule's next turn is undated), AND
 *   - the task has a `scheduled_date` (its base).
 * Everything else passes through unchanged (shown only on its current day).
 *
 * The occurrence at the task's OWN `scheduled_date` is the REAL, interactive
 * task (returned verbatim). Every OTHER occurrence is a read-only PROJECTION: a
 * shallow copy with that day's `scheduled_date` and an occurrence id
 * (`isRecurringProjection` / `recurringSeriesTaskId` decode it) so the calendar
 * chips can render it as a preview (no complete/reschedule — those act on the
 * real current instance, which advances the series on completion) and route a
 * tap to the underlying series.
 *
 * Mirrors the backend spawner (`cal_core::spawn::advance` / `next_trigger`) so
 * the projected days match the instances the backend will actually create.
 */

const OCC_SEP = ' occ ';
// A projection id is EXACTLY the base id followed by ` occ ` + an ISO date, at
// the very end. Matching the whole suffix shape (not just the ` occ ` substring)
// means a real task id that merely CONTAINS ` occ ` somewhere isn't misclassified
// as a projection — only one that literally ends in ` occ YYYY-MM-DD` could
// collide, which no real UUID / provider UID does.
const OCC_SUFFIX_RE = / occ \d{4}-\d{2}-\d{2}$/;

/** Occurrence id for a projected instance of `baseId` on `dateKey`. */
export function makeOccurrenceId(baseId: string, dateKey: string): string {
  return `${baseId}${OCC_SEP}${dateKey}`;
}

/** True when `id` (or a task's id) is a projected recurring-task occurrence. */
export function isRecurringProjection(idOrTask: string | { id: string }): boolean {
  const id = typeof idOrTask === 'string' ? idOrTask : idOrTask.id;
  return OCC_SUFFIX_RE.test(id);
}

/** The underlying series/task id for a (possibly projected) task id — strips the
 *  occurrence suffix so an action on a projection routes to the real task. A
 *  no-op for a real task id. */
export function recurringSeriesTaskId(idOrTask: string | { id: string }): string {
  const id = typeof idOrTask === 'string' ? idOrTask : idOrTask.id;
  return id.replace(OCC_SUFFIX_RE, '');
}

// ── Pure calendar-date arithmetic (UTC-noon anchored, so no DST drift) ───────

function parseKey(key: string): Date {
  const [y, m, d] = key.split('-').map(Number);
  return new Date(Date.UTC(y, m - 1, d));
}

function fmtKey(d: Date): string {
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, '0');
  const day = String(d.getUTCDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

function addDays(d: Date, n: number): Date {
  const r = new Date(d);
  r.setUTCDate(r.getUTCDate() + n);
  return r;
}

function clampToMonth(year: number, month0: number, day: number): Date {
  const last = new Date(Date.UTC(year, month0 + 1, 0)).getUTCDate();
  return new Date(Date.UTC(year, month0, Math.min(Math.max(1, day), last)));
}

/** Add `n` months, clamping the day to the target month's length (Jan 31 + 1
 *  month → Feb 28/29) — matches Rust's `checked_add_months`. */
function addMonths(d: Date, n: number): Date {
  const total = d.getUTCMonth() + n;
  const year = d.getUTCFullYear() + Math.floor(total / 12);
  const month0 = ((total % 12) + 12) % 12;
  return clampToMonth(year, month0, d.getUTCDate());
}

const ISO_WEEKDAY: Record<string, number> = {
  MO: 1, TU: 2, WE: 3, TH: 4, FR: 5, SA: 6, SU: 7,
};

/** ISO weekday (1=Mon..7=Sun) of a UTC date. */
function isoWeekday(d: Date): number {
  const day = d.getUTCDay(); // 0=Sun..6=Sat
  return day === 0 ? 7 : day;
}

/** The next listed weekday after `anchor`: within the anchor's own week if one
 *  is left there, otherwise the first listed day of the week `intervalWeeks`
 *  later. Weeks start on MONDAY (RFC 5545's default `WKST`, not the user's
 *  display setting). Mirrors Rust's `next_weekday_after` — including why it
 *  counts weeks: scanning the next seven days can never reach further than
 *  seven days, so "every 2 weeks on Monday" advanced by one week and the
 *  interval was silently dropped whenever the rule named its weekdays. */
function nextWeekdayAfter(
  anchor: Date,
  byDay: string[],
  intervalWeeks: number,
): Date | null {
  const allowed = byDay.map((d) => ISO_WEEKDAY[d]).filter(Boolean);
  if (allowed.length === 0) return null;
  const weekStart = addDays(anchor, -(isoWeekday(anchor) - 1));
  // A listed day still to come this week wins, whatever the interval.
  const restOfWeek = allowed
    .map((iso) => addDays(weekStart, iso - 1))
    .filter((cand) => cand.getTime() > anchor.getTime())
    .sort((a, b) => a.getTime() - b.getTime());
  if (restOfWeek.length > 0) return restOfWeek[0];
  const nextBlock = addDays(weekStart, 7 * Math.max(1, intervalWeeks));
  return addDays(nextBlock, Math.min(...allowed) - 1);
}

/** Earliest fixed (month, day) trigger strictly after `from`, scanning this
 *  year then next; clamps the day to the month. Mirrors `next_fixed_date_after`. */
function nextFixedDateAfter(
  from: Date,
  dates: { month: number; day: number }[],
): Date | null {
  let best: Date | null = null;
  for (const year of [from.getUTCFullYear(), from.getUTCFullYear() + 1]) {
    for (const md of dates) {
      if (md.month < 1 || md.month > 12) continue;
      const cand = clampToMonth(year, md.month - 1, md.day);
      if (
        cand.getTime() > from.getTime() &&
        (best === null || cand.getTime() < best.getTime())
      ) {
        best = cand;
      }
    }
  }
  return best;
}

/** One step forward by frequency × interval (or day-of-month / weekday snap).
 *  Mirrors Rust's `advance`. */
function advance(date: Date, rule: TaskRecurrenceValue): Date | null {
  const interval = Math.max(1, rule.interval);
  switch (rule.freq) {
    case 'DAILY':
      return addDays(date, interval);
    case 'WEEKLY':
      if (rule.byDay.length > 0) return nextWeekdayAfter(date, rule.byDay, interval);
      return addDays(date, 7 * interval);
    case 'MONTHLY': {
      const next = addMonths(date, interval);
      if (rule.dayOfMonth > 0) {
        return clampToMonth(next.getUTCFullYear(), next.getUTCMonth(), rule.dayOfMonth);
      }
      return next;
    }
    case 'YEARLY':
      return addMonths(date, 12 * interval);
    default:
      return null;
  }
}

/** The next trigger after `base` — the next fixed date when set, else `advance`.
 *  Mirrors Rust's `next_trigger`. */
function nextTrigger(base: Date, rule: TaskRecurrenceValue): Date | null {
  if (rule.fixedDates.length > 0) return nextFixedDateAfter(base, rule.fixedDates);
  return advance(base, rule);
}

/** True once a date-bounded (`UNTIL`) rule has run past its end. */
function ended(rule: TaskRecurrenceValue, dateKey: string): boolean {
  return rule.endMode === 'UNTIL' && rule.until !== '' && dateKey > rule.until;
}

/** Hard cap on the total per-task walk (advance steps), independent of how many
 *  occurrences are emitted — guarantees termination even for a base date very far
 *  from the window. 100k daily steps ≈ 270 years, far beyond any real gap. */
const MAX_STEPS = 100_000;

/**
 * Replace each expandable recurring scheduled task with its occurrences inside
 * `[fromKey, toKey]` (inclusive, `YYYY-MM-DD` keys). Non-expandable tasks pass
 * through verbatim. `maxPerTask` caps the number of RENDERED occurrences per task
 * over a wide window (a separate step cap bounds the walk to reach the window).
 */
export function expandScheduledRecurringTasks<T extends Task>(
  tasks: T[],
  fromKey: string,
  toKey: string,
  maxPerTask = 400,
): T[] {
  const out: T[] = [];
  for (const task of tasks) {
    const rule = expandableRule(task);
    if (rule === null || task.scheduled_date == null) {
      out.push(task);
      continue;
    }
    const baseKey = task.scheduled_date;
    let date = parseKey(baseKey);
    let dateKey = baseKey;
    // `emitted` bounds the number of RENDERED occurrences (the point of
    // maxPerTask). `steps` is a separate CPU guard on the total walk: a series
    // whose stored `scheduled_date` sits far BEFORE the window (an old, never-
    // completed daily task keeps its original date — only completion advances it)
    // must be able to walk forward INTO the window without the emit budget being
    // spent on the out-of-window days it skips over. Counting iterations against
    // maxPerTask (as before) made such a task hit the cap before reaching the
    // window and vanish entirely; counting only emissions fixes that, and
    // MAX_STEPS still guarantees termination (100k daily steps ≈ 270 years).
    let emitted = 0;
    let steps = 0;
    while (dateKey <= toKey && emitted < maxPerTask && steps < MAX_STEPS) {
      // An `UNTIL` end is monotonic (dates only advance), so once past it
      // nothing later can emit — stop, which also caps the walk of a far-past
      // UNTIL-bounded series.
      if (ended(rule, dateKey)) break;
      if (dateKey >= fromKey) {
        if (dateKey === baseKey) {
          out.push(task); // the real, interactive current instance
        } else {
          out.push(projectionOf(task, dateKey));
        }
        emitted += 1;
      }
      const next = nextTrigger(date, rule);
      if (next === null) break;
      const nextKey = fmtKey(next);
      if (nextKey === dateKey) break; // guard against a non-advancing rule
      date = next;
      dateKey = nextKey;
      steps += 1;
    }
  }
  return out;
}

/** The next scheduled occurrence key (`YYYY-MM-DD`) strictly after `scheduledKey`
 *  for a repeating task, or `null` when the rule has no next step or the series
 *  has run past its `UNTIL` end. Reuses the same `advance`/`ended` walk the
 *  projector uses. Used by "skip only this occurrence" on a device reminder,
 *  where the current due date is rolled forward one step instead of deleting the
 *  series (COUNT-bounded rules aren't tracked here — the provider still owns the
 *  count). */
export function nextTaskOccurrence(
  scheduledKey: string,
  rule: TaskRecurrenceValue,
): string | null {
  const next = nextTrigger(parseKey(scheduledKey), rule);
  if (next === null) return null;
  const key = fmtKey(next);
  if (key === scheduledKey) return null; // non-advancing rule guard
  if (ended(rule, key)) return null;
  return key;
}

/** The parsed rule IF this task's recurrence is deterministic + dated +
 *  scheduled (so it can be projected); otherwise `null`. */
function expandableRule(task: Task): TaskRecurrenceValue | null {
  // A terminal instance never projects: a completed / cancelled recurring task
  // shows only on its own day — the FUTURE occurrences belong to the next OPEN
  // instance the backend spawns on completion. Without this, every past
  // completed daily instance projects forward onto today (e.g. a "take pills"
  // task appearing once per past completion — 15 times at 08:30).
  if (task.status === 'completed' || task.status === 'cancelled') return null;
  if (task.recurrence == null) return null;
  const rule = fromBackend(task.recurrence);
  if (rule.freq === 'NONE') return null;
  if (rule.anchor !== 'FROM_DATE') return null;
  if (rule.placement !== 'SCHEDULE') return null;
  return rule;
}

/** A read-only projected copy of `task` on `dateKey`. */
function projectionOf<T extends Task>(task: T, dateKey: string): T {
  return {
    ...task,
    id: makeOccurrenceId(task.id, dateKey),
    scheduled_date: dateKey,
    // The time-of-day (if any) rides each occurrence.
    scheduled_time: task.scheduled_time,
  };
}

/**
 * What "move this task to `dateKey`" can actually be, given what its source
 * supports.
 *
 * Most backends store the due date on the task, so the answer is simply the day
 * asked for. iOS Reminders does not: there the date is the SERIES anchor, and
 * an arbitrary day written to a repeating reminder does not survive the round
 * trip. It failed silently — the day-start carry-over reported success, the
 * reminder kept its old date, and the day it had been carried to went on
 * showing a read-only preview with no checkbox, which is the shape the bug
 * arrived in.
 *
 * Where the source cannot do it, the honest equivalent is to advance the series
 * by one step — the same move "skip this occurrence" already makes for the same
 * adapters. `advanced` says which of the two happened, so the caller can tell
 * the user rather than quietly doing something else than was asked.
 *
 * Returns the requested day unchanged when there is no later turn (a series
 * that has ended): failing to move is better than moving somewhere invented.
 */
export function occurrenceMoveTarget(
  task: Task,
  dateKey: string | null,
  canRescheduleOccurrence: boolean,
): { date: string | null; advanced: boolean } {
  if (dateKey == null || canRescheduleOccurrence) {
    return { date: dateKey, advanced: false };
  }
  if (task.recurrence == null || task.scheduled_date == null) {
    return { date: dateKey, advanced: false };
  }
  const next = nextTaskOccurrence(task.scheduled_date, fromBackend(task.recurrence));
  return next == null ? { date: dateKey, advanced: false } : { date: next, advanced: true };
}
