import { describe, expect, it } from 'vitest';

import type { TaskCapabilities, TaskList } from '../api/types';
import {
  canAssignSection,
  canMoveTaskBetweenLists,
  canReparentList,
  createCapableAccounts,
  reparentCandidates,
  supportsNestedProjects,
} from './taskMoves';

const list = (
  id: string,
  accountId: string,
  parentId: string | null = null,
  caps?: Partial<TaskCapabilities>,
): TaskList => ({
  id,
  name: id,
  color: null,
  default_sound: null,
  embedded_in_calendar: null,
  read_only: false,
  account_id: accountId,
  parent_id: parentId,
  task_capabilities: caps
    ? ({
        nested_projects: false,
        subtasks: true,
        max_subtask_depth: null,
        sections: false,
        multiple_labels: false,
        task_recurrence: true,
        move_between_projects: true,
        ...caps,
      } as TaskCapabilities)
    : undefined,
});

describe('capability predicates', () => {
  it('move-between-lists defaults to true and respects the flag', () => {
    expect(canMoveTaskBetweenLists(list('a', 'acc'))).toBe(true);
    expect(
      canMoveTaskBetweenLists(list('a', 'acc', null, { move_between_projects: false })),
    ).toBe(false);
  });

  it('section assignment is gated on the sections capability', () => {
    expect(canAssignSection(list('a', 'acc'))).toBe(false); // default
    expect(canAssignSection(list('a', 'acc', null, { sections: true }))).toBe(true);
  });

  it('nested-projects support reads the capability', () => {
    expect(supportsNestedProjects(list('a', 'acc'))).toBe(false);
    expect(
      supportsNestedProjects(list('a', 'acc', null, { nested_projects: true })),
    ).toBe(true);
  });
});

describe('canReparentList', () => {
  const nest = (id: string, account = 'acc', parent: string | null = null) =>
    list(id, account, parent, { nested_projects: true });

  it('rejects when the adapter does not nest projects', () => {
    const flat = list('a', 'acc');
    expect(canReparentList('a', 'b', [flat, list('b', 'acc')])).toBe(false);
  });

  it('allows promoting to top level (null parent)', () => {
    expect(canReparentList('a', null, [nest('a', 'acc', 'b'), nest('b')])).toBe(
      true,
    );
  });

  it('allows a valid same-account reparent', () => {
    expect(canReparentList('a', 'b', [nest('a'), nest('b')])).toBe(true);
  });

  it('rejects making a list its own parent', () => {
    expect(canReparentList('a', 'a', [nest('a')])).toBe(false);
  });

  it('rejects crossing account boundaries', () => {
    expect(
      canReparentList('a', 'b', [nest('a', 'acc1'), nest('b', 'acc2')]),
    ).toBe(false);
  });

  it('rejects a cycle (nesting a list under its own descendant)', () => {
    // a → b → c. Reparenting a under c would form a cycle.
    const lists = [nest('a'), nest('b', 'acc', 'a'), nest('c', 'acc', 'b')];
    expect(canReparentList('a', 'c', lists)).toBe(false);
    // The reverse (c under a) is fine — a is not a descendant of c.
    expect(canReparentList('c', 'a', lists)).toBe(true);
  });
});

describe('createCapableAccounts', () => {
  const acct = (id: string, display_name: string) => ({ id, display_name });

  it('always offers local first, then create-capable external accounts', () => {
    const lists = [
      list('v1', 'vik', null, { create_lists: true }),
      list('t1', 'todo', null, { create_lists: false }),
    ];
    const accounts = [
      acct('local', 'On this device'),
      acct('vik', 'Vikunja'),
      acct('todo', 'Todoist'),
    ];
    const result = createCapableAccounts(lists, accounts, 'local', 'Local');
    // Local first; Vikunja included (create_lists), Todoist excluded.
    expect(result.map((a) => a.id)).toEqual(['local', 'vik']);
    expect(result[0].name).toBe('On this device');
  });

  it('falls back to the supplied local name when absent from accounts', () => {
    const result = createCapableAccounts([], [], 'local', 'Local');
    expect(result).toEqual([{ id: 'local', name: 'Local' }]);
  });
});

describe('reparentCandidates', () => {
  const nest = (id: string, account = 'acc', parent: string | null = null) =>
    list(id, account, parent, { nested_projects: true });

  it('is empty for a flat (non-nesting) adapter', () => {
    expect(reparentCandidates('a', [list('a', 'acc'), list('b', 'acc')])).toEqual(
      [],
    );
  });

  it('excludes self, the current parent, descendants and other accounts', () => {
    // a → b → c, plus a sibling d, plus e on another account.
    const lists = [
      nest('a'),
      nest('b', 'acc', 'a'),
      nest('c', 'acc', 'b'),
      nest('d'),
      nest('e', 'other'),
    ];
    // Candidates for b: NOT itself, NOT its current parent (a), NOT its
    // descendant (c → cycle), NOT cross-account (e). Only d remains.
    expect(reparentCandidates('b', lists).map((l) => l.id)).toEqual(['d']);
  });
});
