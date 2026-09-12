// Test support for the taskStatus contract: the two answers that move into
// the core — the i18n KEY tables and the subtask progress — reduced from
// today's TypeScript. Shared by the contract test and the one-off measure
// test; nothing in the app imports this.
import type { Task, TaskEffort, TaskPriority, TaskStatus } from '../api/types';
import {
  effortI18nKey,
  priorityI18nKey,
  statusI18nKey,
  subtaskProgress,
  type PriorityScale,
} from '@aperio/shared';

const STATUSES: TaskStatus[] = ['open', 'in_progress', 'completed', 'cancelled'];
const EFFORTS: TaskEffort[] = ['small', 'medium', 'large'];
const PRIORITIES: TaskPriority[] = ['low', 'medium', 'high'];
const SCALES: PriorityScale[] = ['three', 'two'];

/** The i18n keys the core will publish, once, for every enum value. `null`
 *  where there is nothing to announce. */
export interface KeyTable {
  status: Record<TaskStatus, string>;
  effort: Record<TaskEffort, string | null>;
  priority: Record<PriorityScale, Record<TaskPriority, string | null>>;
}

export function answerKeys(): KeyTable {
  return {
    status: Object.fromEntries(STATUSES.map((s) => [s, statusI18nKey(s)])) as Record<
      TaskStatus,
      string
    >,
    effort: Object.fromEntries(EFFORTS.map((e) => [e, effortI18nKey(e)])) as Record<
      TaskEffort,
      string | null
    >,
    priority: Object.fromEntries(
      SCALES.map((scale) => [
        scale,
        Object.fromEntries(PRIORITIES.map((p) => [p, priorityI18nKey(p, scale)])),
      ]),
    ) as Record<PriorityScale, Record<TaskPriority, string | null>>,
  };
}

export interface ProgressInput {
  /** Task overrides over the fixture's base task. */
  tasks: Partial<Task>[];
  /** The parents asked about. */
  parents: string[];
}

/** Parent id → {done, total}, or null when it has no children that count. */
export type ProgressAnswer = Record<string, { done: number; total: number } | null>;

export function answerProgress(input: ProgressInput, baseTask: Task): ProgressAnswer {
  const tasks = input.tasks.map((over) => ({ ...baseTask, ...over }) as Task);
  const out: ProgressAnswer = {};
  for (const parent of input.parents) out[parent] = subtaskProgress(parent, tasks);
  return out;
}
