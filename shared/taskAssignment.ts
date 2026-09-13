// The assignment rules — this surface's door into `cal_core::task_assignment`.
//
// Who holds a task after a status change (take it when starting or finishing
// a task nobody owns, step back when reopening one I hold), how many people a
// list can hold on one task, and what a list of assignees is trimmed to: that
// decision lives in the core now, and both surfaces ask it when a task is
// checked off or moved. What stays here is the shell — building the question
// from what a caller holds (a user is its id; the rule compares ids alone) and
// laying the answer (positions, or "take me") back over the caller's own user
// rows — and the one rule that already was the core's and still has a
// TypeScript reader (`isMineOrUnassigned`, below).
//
// Pinned by `crates/cal-core/tests/fixtures/taskAssignment.json`, measured
// from the TypeScript this replaced; `taskAssignment.contract.test.ts` replays
// it through this door, and the core's own contract test reads the same file.

import type {
  AssignmentModeInput,
  ClampAssigneesInput,
  SelfAssignInput,
  SelfAssignOutcome,
  TaskAssignment,
  TaskCapabilities,
  TaskStatus,
  TaskUser,
} from './types';

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::task_assignment`. */
export interface TaskAssignmentRules {
  selfAssignOnStatusChangeJson(inputJson: string): string;
  taskAssignmentModeJson(inputJson: string): string;
  clampAssigneesJson(inputJson: string): string;
}

let installedRules: TaskAssignmentRules | null = null;

/** Bind this surface's door into the core. */
export function installTaskAssignmentRules(rules: TaskAssignmentRules): void {
  installedRules = rules;
}

function rules(): TaskAssignmentRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the copy all over
    // again, and its failure is one nobody reports: a task taken on one
    // device and not on the other.
    throw new Error(
      'task assignment rules used before installTaskAssignmentRules() — the ' +
        'surface must bind its door into cal_core::task_assignment at startup',
    );
  }
  return installedRules;
}

/**
 * The new assignee list for a status transition, or `undefined` when nothing
 * should change. In shared lists where the adapter knows "me":
 *   - →`in_progress` / →`completed` on an UNASSIGNED task ⇒ `[me]`
 *   - →`open` while I'm an assignee                       ⇒ assignees minus me
 *
 * Only fires when `enabled` and a `me` identity exists for the task's list. The
 * reopen path removes ONLY me (a colleague's assignment survives), symmetric to
 * the auto-assign, which only acts on a task nobody owns. `cancelled` and
 * already-correct transitions are left untouched.
 *
 * The core answers with positions (which of the given assignees stay) or with
 * "take me"; the rows handed back are the caller's own.
 */
export function selfAssignOnStatusChange(
  nextStatus: TaskStatus,
  assignees: TaskUser[],
  me: TaskUser | null,
  enabled: boolean,
): TaskUser[] | undefined {
  const input: SelfAssignInput = {
    status: nextStatus,
    assignees: assignees.map((a) => a.id),
    me: me === null ? null : me.id,
    enabled,
  };
  const outcome = JSON.parse(
    rules().selfAssignOnStatusChangeJson(JSON.stringify(input)),
  ) as SelfAssignOutcome;
  switch (outcome.change) {
    case 'unchanged':
      return undefined;
    case 'assign_me':
      // `me` is present whenever the core answers this: no identity, no change.
      return me === null ? undefined : [me];
    case 'keep':
      return outcome.positions.map((i) => assignees[i]);
  }
}

/**
 * True when a task is "mine to act on": there's no identity, OR it's unassigned,
 * OR I'm one of the assignees. False ONLY when it's assigned to concrete OTHER
 * users and not me.
 *
 * The rule is the core's (`cal_core::is_mine_or_unassigned`): the reminder
 * scheduler, the calendar day and the task grouping ask it there. This copy
 * serves `shared/dayStart.ts`, which walks every task at day start, until the
 * day-start rules move as well; it is pinned against the core by
 * `shared/contracts/taskOwnership.json`, which both test suites read.
 */
export function isMineOrUnassigned(assignees: TaskUser[], me: TaskUser | null): boolean {
  return !me || assignees.length === 0 || assignees.some((a) => a.id === me.id);
}

/**
 * How many people a task on `list` may be assigned to.
 *
 * ONE implementation for every surface, because the editors would otherwise
 * each decide it — and this is the rule that stops the editor offering a
 * choice the source cannot keep. Absent capabilities mean `none`, the core's
 * `TaskAssignment::default()`: an adapter that has not said it can assign is
 * taken at its word rather than credited with an ability whose failure is
 * silent.
 */
export function taskAssignmentMode(
  list: { task_capabilities?: TaskCapabilities } | undefined,
): TaskAssignment {
  const input: AssignmentModeInput = { capabilities: list?.task_capabilities ?? null };
  return JSON.parse(rules().taskAssignmentModeJson(JSON.stringify(input))) as TaskAssignment;
}

/**
 * Trim `assignees` to what `mode` can actually hold, keeping the FIRST — the
 * same one Todoist's adapter keeps when it clamps (`first_assignee_id`), so
 * the editor and the wire agree about who survives.
 *
 * Needed when a task MOVES: carry a two-assignee Vikunja task to a Todoist
 * list and the form still holds both while the picker can show only one, so
 * the save would send two and the adapter would quietly drop one.
 *
 * `none` is deliberately NOT emptied. The editor simply does not show the
 * picker there; the task may still carry assignees written by another client,
 * and clearing them on open would destroy what the user was never shown —
 * which is the whole failure this capability exists to stop.
 */
export function clampAssignees(
  mode: TaskAssignment,
  assignees: TaskUser[],
): TaskUser[] {
  const input: ClampAssigneesInput = { mode, assignees: assignees.map((a) => a.id) };
  const positions = JSON.parse(rules().clampAssigneesJson(JSON.stringify(input))) as number[];
  return positions.map((i) => assignees[i]);
}
