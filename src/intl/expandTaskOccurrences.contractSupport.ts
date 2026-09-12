// Test support for the expandTaskOccurrences contract: the three answers of
// shared/expandTaskOccurrences.ts that move into the core, reduced to plain
// data in and plain data out. Shared by the contract test and the one-off
// measure test; nothing in the app imports this.
import type { Task, TaskRecurrence } from '../api/types';
import {
  expandScheduledRecurringTasks,
  fromBackend,
  isRecurringProjection,
  nextTaskOccurrence,
  occurrenceMoveTarget,
  recurringSeriesTaskId,
} from '@aperio/shared';

export interface ExpandInput {
  /** Task overrides over the fixture's base task; `recurrence` in the wire
   *  shape (`cal_core::TaskRecurrence`), as `Task.recurrence` holds it. */
  tasks: Partial<Task>[];
  from: string;
  to: string;
  /** Omitted = the default cap. */
  maxPerTask?: number;
}

/** One row of the answer: which input task, on which day, real or projected.
 *  A pass-through row carries the task's own `scheduled_date` (may be null). */
export interface OccurrenceRow {
  task: number;
  day: string | null;
  projection: boolean;
}

export function answerExpand(input: ExpandInput, baseTask: Task): OccurrenceRow[] {
  const tasks = input.tasks.map((over) => ({ ...baseTask, ...over }) as Task);
  const out =
    input.maxPerTask === undefined
      ? expandScheduledRecurringTasks(tasks, input.from, input.to)
      : expandScheduledRecurringTasks(tasks, input.from, input.to, input.maxPerTask);
  return out.map((row) => {
    const real = tasks.indexOf(row);
    // A pass-through keeps the caller's object; its day is the one it shows
    // on, and an empty string is no day (the shell reads it that way too).
    if (real >= 0) return { task: real, day: row.scheduled_date || null, projection: false };
    if (!isRecurringProjection(row)) throw new Error(`neither real nor projected: ${row.id}`);
    const series = recurringSeriesTaskId(row.id);
    const idx = tasks.findIndex((t) => t.id === series);
    if (idx < 0) throw new Error(`projection of an unknown series: ${row.id}`);
    return { task: idx, day: row.scheduled_date, projection: true };
  });
}

export interface NextInput {
  scheduled: string;
  /** The wire shape, or null (no rule). */
  rule: TaskRecurrence | null;
}

export function answerNext(input: NextInput): string | null {
  return nextTaskOccurrence(input.scheduled, fromBackend(input.rule));
}

export interface MoveInput {
  task: Partial<Task>;
  dateKey: string | null;
  canRescheduleOccurrence: boolean;
}

export function answerMove(
  input: MoveInput,
  baseTask: Task,
): { date: string | null; advanced: boolean } {
  return occurrenceMoveTarget(
    { ...baseTask, ...input.task } as Task,
    input.dateKey,
    input.canRescheduleOccurrence,
  );
}
