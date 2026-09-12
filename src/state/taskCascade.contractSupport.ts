// Test support for the taskCascade contract: the four answers of
// shared/taskCascade.ts that move into the core, reduced to plain data in and
// plain data out. Shared by the contract test and the one-off measure test;
// nothing in the app imports this.
import type { Task, TaskStatus } from '../api/types';
import {
  autoDateOnStart,
  planAncestorRecompute,
  planStatusCascade,
  type StatusWrite,
} from '@aperio/shared';

/** `null` stands for "omitted" on the wire: no today key, no date. */
export interface AutoDateInput {
  newStatus: TaskStatus;
  currentScheduledDate: string | null;
  todayKey: string | null;
}

export function answerAutoDate(input: AutoDateInput): string | null {
  return (
    autoDateOnStart(
      input.newStatus,
      input.currentScheduledDate,
      input.todayKey ?? undefined,
    ) ?? null
  );
}

export interface DeriveInput {
  /** One child per entry, in this order. */
  children: TaskStatus[];
}

/**
 * The derivation has no door of its own (no caller needs it alone), so it is
 * observed through the recompute: a parent with these children, asked twice.
 * Asked as `open`, the recompute writes the derived status unless that is
 * `open` itself (or there are no children); asked as `cancelled`, it writes
 * `open` when the derivation is `open`. Together the two answers name every
 * derivation, and `null` falls out when neither writes.
 */
export function answerDerive(input: DeriveInput, baseTask: Task): TaskStatus | null {
  const rows = (parent: TaskStatus): Task[] => [
    { ...baseTask, id: 'p', parent_id: null, status: parent },
    ...input.children.map((status, i) => ({
      ...baseTask,
      id: `c${i}`,
      parent_id: 'p',
      status,
    })),
  ];
  const writeFor = (parent: TaskStatus): TaskStatus | undefined =>
    planAncestorRecompute('p', rows(parent))[0]?.status;
  return writeFor('open') ?? writeFor('cancelled') ?? null;
}

export interface CascadeOptionsInput {
  cascadeEnabled?: boolean;
  /** `null` = omitted (the auto-date setting is off). */
  todayKey?: string | null;
}

export interface CascadeInput {
  /** Task overrides over the fixture's base task. */
  tasks: Partial<Task>[];
  taskId: string;
  newStatus: TaskStatus;
  options?: CascadeOptionsInput;
}

export interface RecomputeInput {
  tasks: Partial<Task>[];
  parentId: string;
  options?: CascadeOptionsInput;
}

/** A write as the fixture spells it: `scheduledDate` only when set. */
export interface WriteAnswer {
  taskId: string;
  status: TaskStatus;
  scheduledDate?: string;
}

function options(input: CascadeOptionsInput | undefined) {
  if (input === undefined) return undefined;
  return {
    ...(input.cascadeEnabled === undefined ? {} : { cascadeEnabled: input.cascadeEnabled }),
    ...(input.todayKey === undefined || input.todayKey === null
      ? {}
      : { todayKey: input.todayKey }),
  };
}

function plain(writes: StatusWrite[]): WriteAnswer[] {
  return writes.map((w) =>
    w.scheduledDate === undefined
      ? { taskId: w.taskId, status: w.status }
      : { taskId: w.taskId, status: w.status, scheduledDate: w.scheduledDate },
  );
}

export function answerCascade(input: CascadeInput, baseTask: Task): WriteAnswer[] {
  const tasks = input.tasks.map((over) => ({ ...baseTask, ...over }) as Task);
  return plain(planStatusCascade(input.taskId, input.newStatus, tasks, options(input.options)));
}

export function answerRecompute(input: RecomputeInput, baseTask: Task): WriteAnswer[] {
  const tasks = input.tasks.map((over) => ({ ...baseTask, ...over }) as Task);
  return plain(planAncestorRecompute(input.parentId, tasks, options(input.options)));
}
