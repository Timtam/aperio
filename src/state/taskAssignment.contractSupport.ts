// Test support for the taskAssignment contract: the three answers of
// shared/taskAssignment.ts that move into the core, reduced to plain data in
// and plain data out. Shared by the contract test and the one-off measure
// test; nothing in the app imports this.
import type { TaskStatus, TaskUser } from '../api/types';
import {
  clampAssignees,
  selfAssignOnStatusChange,
  taskAssignmentMode,
  type TaskAssignment,
} from '@aperio/shared';

/** A user is its id here; the rule compares ids alone. */
export function user(id: string): TaskUser {
  return { id, name: `Name of ${id}`, email: null };
}

export interface SelfAssignInput {
  nextStatus: TaskStatus;
  /** Assignee ids, in order. */
  assignees: string[];
  /** The account's own id, or null when the adapter reports no identity. */
  me: string | null;
  enabled: boolean;
}

/** The new assignee ids, or null when nothing should change. */
export function answerSelfAssign(input: SelfAssignInput): string[] | null {
  const out = selfAssignOnStatusChange(
    input.nextStatus,
    input.assignees.map(user),
    input.me === null ? null : user(input.me),
    input.enabled,
  );
  return out === undefined ? null : out.map((u) => u.id);
}

export interface ModeInput {
  /** The list's capabilities as the wire carries them, or null for a list
   *  without any; `absent` when the list itself is unknown. */
  capabilities: Record<string, unknown> | null | 'absent';
}

export function answerMode(input: ModeInput): TaskAssignment {
  if (input.capabilities === 'absent') return taskAssignmentMode(undefined);
  return taskAssignmentMode({
    task_capabilities:
      input.capabilities === null ? undefined : (input.capabilities as never),
  });
}

export interface ClampInput {
  mode: TaskAssignment;
  assignees: string[];
}

export function answerClamp(input: ClampInput): string[] {
  return clampAssignees(input.mode, input.assignees.map(user)).map((u) => u.id);
}
