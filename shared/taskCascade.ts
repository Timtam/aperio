// The parent/subtask status coupling — this surface's door into
// `cal_core::task_cascade`.
//
// What a check-off writes — the root, the descendants that follow, the
// ancestors re-derived, in application order — and the "started → pin to
// today" companion date: that decision lives in the core now, and both
// surfaces ask it when a task changes status (DESIGN §9.1). What stays here is
// the shell — building the question from what a caller holds and giving the
// answer back in the shape the callers have always read (`StatusWrite`,
// `CascadeOptions`) — and everything the core deliberately does not do: read
// the clock (`todayIsoKey`, the caller's), decide whether a today key travels
// at all (the auto-date setting, a provider that cannot hold `in_progress`,
// the per-list rule), apply the writes and announce the outcome.
//
// Pinned by `crates/cal-core/tests/fixtures/taskCascade.json`, measured from
// the TypeScript this replaced; `taskCascade.contract.test.ts` replays it
// through this door, and the core's own contract test reads the same file.

import type {
  AutoDateInput,
  CascadeInput,
  RecomputeInput,
  StatusWriteWire,
  Task,
  TaskStatus,
} from './types';

export interface StatusWrite {
  taskId: string;
  status: TaskStatus;
  /**
   * Companion write: when set, the task's `scheduled_date` should
   * also be updated to this ISO date string (`YYYY-MM-DD`). Used by
   * the "started → pin to today" auto-date feature: a task that
   * transitions into `in_progress` while it currently has no
   * scheduled_date gets it pinned so the missed-tasks / carry-over
   * flow can find it later. Independent of the cascade-coupling
   * preference — the date logic applies even when coupling is off.
   */
  scheduledDate?: string;
}

/**
 * Behaviour options shared by both planners.
 *
 * `cascadeEnabled` defaults to `true` — the historical "couple parent
 * and subtask status" behaviour. Users can disable it from the Tasks
 * settings tab; the planners then degrade to a single-row write
 * (`planStatusCascade`) or a no-op (`planAncestorRecompute`).
 *
 * `todayKey` enables the auto-date feature documented on `StatusWrite`.
 * When provided, every write the planner emits for a transition into
 * `in_progress` on a dateless task carries the date as a companion
 * change. Passed in (rather than read inline) so the core stays free of
 * the clock.
 */
export interface CascadeOptions {
  cascadeEnabled?: boolean;
  todayKey?: string;
}

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::task_cascade`. */
export interface TaskCascadeRules {
  planStatusCascadeJson(inputJson: string): string;
  planAncestorRecomputeJson(inputJson: string): string;
  autoDateOnStartJson(inputJson: string): string;
}

let installedRules: TaskCascadeRules | null = null;

/** Bind this surface's door into the core. */
export function installTaskCascadeRules(rules: TaskCascadeRules): void {
  installedRules = rules;
}

function rules(): TaskCascadeRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // planner all over again, and its failure is the one that made this move
    // necessary: a check-off writing one family on the phone and another on
    // the desktop.
    throw new Error(
      'task cascade rules used before installTaskCascadeRules() — the surface ' +
        'must bind its door into cal_core::task_cascade at startup',
    );
  }
  return installedRules;
}

/** Only what the planners read crosses; a task carries far more. */
function wireTasks(allTasks: readonly Task[]): CascadeInput['tasks'] {
  return allTasks.map((t) => ({
    id: t.id,
    status: t.status,
    // "" was no parent in the TypeScript (a truthiness test) and is no parent
    // in the core; sent as null so the answer never depends on which side
    // read it. `scheduled_date` crosses as it is: "" counted as dated, and the
    // fixture pins that (`an-empty-scheduled-date-counts-as-dated`).
    parent_id: t.parent_id || null,
    scheduled_date: t.scheduled_date,
  }));
}

function wireOptions(options: CascadeOptions | undefined): CascadeInput['options'] {
  return {
    cascade_enabled: options?.cascadeEnabled ?? true,
    // "" was no today in the TypeScript; the core reads it the same way.
    today: options?.todayKey || null,
  };
}

function hydrate(writes: StatusWriteWire[]): StatusWrite[] {
  return writes.map((w) =>
    w.scheduled_date === undefined || w.scheduled_date === null
      ? { taskId: w.task_id, status: w.status }
      : { taskId: w.task_id, status: w.status, scheduledDate: w.scheduled_date },
  );
}

/**
 * The "started → pin to today" rule, one definition for every entry point.
 * Returns the date to pin (`todayKey`) when a task moving into `in_progress`
 * has no scheduled day, else `undefined` ("leave the date as-is"):
 *   - the new status is `in_progress`
 *   - a `todayKey` is available (the auto-date setting is on)
 *   - the task currently has no `scheduled_date`
 *
 * Used inside the core by both planners (for the root transition + any
 * ancestor the up-cascade derives to `in_progress`) and here by the task
 * editor's own root write — both mean "the task is now being worked on".
 */
export function autoDateOnStart(
  newStatus: TaskStatus,
  currentScheduledDate: string | null,
  todayKey: string | undefined,
): string | undefined {
  const input: AutoDateInput = {
    status: newStatus,
    scheduled_date: currentScheduledDate,
    today: todayKey || null,
  };
  const answer = JSON.parse(rules().autoDateOnStartJson(JSON.stringify(input))) as
    | string
    | null;
  return answer ?? undefined;
}

/**
 * Plan all status writes for a single root change.
 *
 * `taskId` changes to `newStatus`. Descendants follow per the
 * cascade-down rule; ancestors are recomputed per the cascade-up
 * rule, recursing all the way to the root.
 *
 * The returned list starts with the root change itself, then the
 * descendants, then the ancestors from nearest to furthest. Idempotent
 * writes (status already matches) are omitted. The caller applies them in
 * order.
 */
export function planStatusCascade(
  taskId: string,
  newStatus: TaskStatus,
  allTasks: readonly Task[],
  options?: CascadeOptions,
): StatusWrite[] {
  const input: CascadeInput = {
    tasks: wireTasks(allTasks),
    task_id: taskId,
    status: newStatus,
    options: wireOptions(options),
  };
  return hydrate(
    JSON.parse(rules().planStatusCascadeJson(JSON.stringify(input))) as StatusWriteWire[],
  );
}

/**
 * Recompute ancestors after a non-status mutation (subtask created,
 * subtask deleted). The up-half of `planStatusCascade`, starting directly
 * from the parent — there is no root status change to write — and climbing
 * to the root even past an ancestor that does not change.
 */
export function planAncestorRecompute(
  parentId: string,
  allTasks: readonly Task[],
  options?: CascadeOptions,
): StatusWrite[] {
  const input: RecomputeInput = {
    tasks: wireTasks(allTasks),
    parent_id: parentId,
    options: wireOptions(options),
  };
  return hydrate(
    JSON.parse(rules().planAncestorRecomputeJson(JSON.stringify(input))) as StatusWriteWire[],
  );
}
