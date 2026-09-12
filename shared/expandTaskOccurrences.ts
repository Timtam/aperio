// The recurring-task projection — this surface's door into
// `cal_core::task_occurrences`.
//
// Which occurrences a recurring scheduled task shows inside a window — the
// real task on its own day, read-only projections on every other, everything
// else passed through — one step of the same walk, and what "move to this
// day" can be on a source that owns the date: that decision lives in the core
// now, on the backend spawner's own stepping, and both surfaces (and the
// widget snapshot) ask it while rendering. What stays here is the shell —
// building the question from what a caller holds, and laying the answer
// (positions and days) back over the caller's own rows: the projected copy,
// its id (`<id> occ <day>`), and the decoding of that id back to the series.
//
// Pinned by `crates/cal-core/tests/fixtures/taskOccurrences.json`, measured
// from the TypeScript this replaced; `expandTaskOccurrences.contract.test.ts`
// replays it through this door, and the core's own contract test reads the
// same file.

import { toBackend, type TaskRecurrenceValue } from './taskRecurrence';
import type {
  MoveTarget,
  MoveTargetInput,
  NextOccurrenceInput,
  OccurrenceInput,
  OccurrenceRow,
  Task,
} from './types';

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

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::task_occurrences`. */
export interface TaskOccurrenceRules {
  expandTaskOccurrencesJson(inputJson: string): string;
  nextTaskOccurrenceJson(inputJson: string): string;
  occurrenceMoveTargetJson(inputJson: string): string;
}

let installedRules: TaskOccurrenceRules | null = null;

/** Bind this surface's door into the core. */
export function installTaskOccurrenceRules(rules: TaskOccurrenceRules): void {
  installedRules = rules;
}

function rules(): TaskOccurrenceRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second walk
    // all over again, and its failure is one a screen-reader user meets head
    // on: a repeating task on one set of days on the phone and on another on
    // the desktop.
    throw new Error(
      'task occurrence rules used before installTaskOccurrenceRules() — the ' +
        'surface must bind its door into cal_core::task_occurrences at startup',
    );
  }
  return installedRules;
}

/** Only what the projector reads crosses; a task carries far more. An empty
 *  `scheduled_date` is no date (the pin measured the TypeScript letting it
 *  make the task vanish; the port reads it as undated, on purpose). */
function wireTask(task: Task): OccurrenceInput['tasks'][number] {
  return {
    status: task.status,
    scheduled_date: task.scheduled_date || null,
    recurrence: task.recurrence ?? null,
  };
}

/**
 * Replace each expandable recurring scheduled task with its occurrences inside
 * `[fromKey, toKey]` (inclusive, `YYYY-MM-DD` keys). Non-expandable tasks pass
 * through verbatim. `maxPerTask` caps the number of RENDERED occurrences per
 * task over a wide window (a separate step cap in the core bounds the walk to
 * reach the window).
 *
 * The occurrence at the task's OWN `scheduled_date` is the REAL, interactive
 * task (returned verbatim). Every OTHER occurrence is a read-only PROJECTION:
 * a shallow copy with that day's `scheduled_date` and an occurrence id
 * (`isRecurringProjection` / `recurringSeriesTaskId` decode it) so the
 * calendar chips can render it as a preview and route a tap to the series.
 */
export function expandScheduledRecurringTasks<T extends Task>(
  tasks: T[],
  fromKey: string,
  toKey: string,
  maxPerTask?: number,
): T[] {
  const input: OccurrenceInput = {
    tasks: tasks.map(wireTask),
    from: fromKey,
    to: toKey,
    max_per_task: maxPerTask ?? null,
  };
  const rows = JSON.parse(
    rules().expandTaskOccurrencesJson(JSON.stringify(input)),
  ) as OccurrenceRow[];
  return rows.map((row) => {
    const task = tasks[row.task];
    // A projected row always carries its day; the real task and a pass-through
    // are the caller's own object.
    return row.projection && row.day !== null ? projectionOf(task, row.day) : task;
  });
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

/** The next scheduled occurrence key (`YYYY-MM-DD`) strictly after `scheduledKey`
 *  for a repeating task, or `null` when the rule has no next step or the series
 *  has run past its `UNTIL` end. One step of the walk the projector uses. Used by
 *  "skip only this occurrence" on a device reminder, where the current due date
 *  is rolled forward one step instead of deleting the series (COUNT-bounded rules
 *  aren't tracked here — the provider still owns the count). */
export function nextTaskOccurrence(
  scheduledKey: string,
  rule: TaskRecurrenceValue,
): string | null {
  const input: NextOccurrenceInput = { scheduled: scheduledKey, rule: toBackend(rule) };
  return JSON.parse(rules().nextTaskOccurrenceJson(JSON.stringify(input))) as string | null;
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
  const input: MoveTargetInput = {
    scheduled_date: task.scheduled_date || null,
    recurrence: task.recurrence ?? null,
    date: dateKey,
    can_reschedule_occurrence: canRescheduleOccurrence,
  };
  const answer = JSON.parse(
    rules().occurrenceMoveTargetJson(JSON.stringify(input)),
  ) as MoveTarget;
  return { date: answer.date, advanced: answer.advanced };
}
