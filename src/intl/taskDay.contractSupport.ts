// Test support for the taskDay contract: the wire shape the core will answer
// with, and the reduction of today's TypeScript answers to that shape. Shared
// by the contract test (which replays the fixture) and the one-off measure
// test (which wrote it) — nothing in the app imports this.
import type { Task, TaskUser } from '../api/types';
import {
  backlogWeeks,
  filterTasksOnDay,
  isDeadlineChip,
  splitDeadlinesByWeek,
  taskEndTimeOnDay,
  taskTimeOnDay,
  type BacklogWeeks,
  type PriorityScale,
} from '@aperio/shared';

/** One task on one day, as the core will answer it: the id, and the three
 *  facts a chip is drawn from. */
export interface DayTaskRow {
  id: string;
  /** `HH:MM:SS` when the task has a time on this day, else null. */
  time: string | null;
  /** The end of a planned block on this day, else null. */
  endTime: string | null;
  /** True when the task sits here because of its deadline, not its plan. */
  deadlineChip: boolean;
}

export interface OnDayInput {
  /** Task overrides over the fixture's base task. A completed task also
   *  carries `completed_day`: the LOCAL day of `completed_at`, which the shell
   *  resolves with the caller's zone and hands to the core. Every instant in
   *  the table is noon UTC, so every zone agrees on it. */
  tasks: (Partial<Task> & { completed_day?: string | null })[];
  /** The days asked about, `YYYY-MM-DD`. */
  days: string[];
  /** Lists whose completed tasks are shown (the per-list opt-in). Absent or
   *  empty means none. */
  completedVisible?: string[];
  /** List id → the connected user, for the ownership filter. Absent means
   *  no gating. */
  currentUserByList?: Record<string, TaskUser | null>;
  scale?: PriorityScale;
}

/** Day → its rows, in the order the day shows them. */
export type OnDayAnswer = Record<string, DayTaskRow[]>;

export function answerOnDay(input: OnDayInput, baseTask: Task): OnDayAnswer {
  const tasks = input.tasks.map((over) => ({ ...baseTask, ...over }) as Task);
  const visible = new Set(input.completedVisible ?? []);
  const isCompletedVisible =
    input.completedVisible === undefined ? undefined : (listId: string) => visible.has(listId);
  const users = input.currentUserByList;
  const meFor = users === undefined ? undefined : (listId: string) => users[listId] ?? null;
  const out: OnDayAnswer = {};
  for (const day of input.days) {
    out[day] = filterTasksOnDay(tasks, day, isCompletedVisible, meFor, input.scale ?? 'three').map(
      (task) => ({
        id: task.id,
        time: taskTimeOnDay(task, day),
        endTime: taskEndTimeOnDay(task, day),
        deadlineChip: isDeadlineChip(task, day),
      }),
    );
  }
  return out;
}

export interface WeeksInput {
  today: string;
  weekStartsOn: number;
}

export function answerWeeks(input: WeeksInput): BacklogWeeks {
  return backlogWeeks(input.today, input.weekStartsOn);
}

export interface SplitInput {
  deadlines: { id: string; deadline_date: string | null }[];
  weeks: BacklogWeeks;
}

export interface SplitAnswer {
  thisWeek: string[];
  nextWeek: string[];
  later: string[];
}

export function answerSplit(input: SplitInput): SplitAnswer {
  const split = splitDeadlinesByWeek(input.deadlines, input.weeks);
  return {
    thisWeek: split.thisWeek.map((t) => t.id),
    nextWeek: split.nextWeek.map((t) => t.id),
    later: split.later.map((t) => t.id),
  };
}
