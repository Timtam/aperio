import { describe, expect, it } from 'vitest';

import type { TaskUser } from '../api/types';
import {
  clampAssignees,
  classifyDoneByMe,
  isMineOrUnassigned,
  selfAssignOnStatusChange,
  taskAssignmentMode,
} from '@aperio/shared';

const me: TaskUser = { id: '7', name: 'Me', email: null };
const other: TaskUser = { id: '42', name: 'Colleague', email: null };

describe('selfAssignOnStatusChange', () => {
  it('assigns me when an unassigned task goes in-progress or done', () => {
    expect(selfAssignOnStatusChange('in_progress', [], me, true)).toEqual([me]);
    expect(selfAssignOnStatusChange('completed', [], me, true)).toEqual([me]);
  });

  it('does not re-assign a task that already has assignees', () => {
    expect(selfAssignOnStatusChange('completed', [other], me, true)).toBeUndefined();
    expect(selfAssignOnStatusChange('completed', [me], me, true)).toBeUndefined();
  });

  it('removes only me when reopening, leaving others', () => {
    expect(selfAssignOnStatusChange('open', [me], me, true)).toEqual([]);
    expect(selfAssignOnStatusChange('open', [me, other], me, true)).toEqual([other]);
  });

  it('leaves a reopened task untouched when I am not an assignee', () => {
    expect(selfAssignOnStatusChange('open', [other], me, true)).toBeUndefined();
    expect(selfAssignOnStatusChange('open', [], me, true)).toBeUndefined();
  });

  it('does nothing for cancelled, when disabled, or without an identity', () => {
    expect(selfAssignOnStatusChange('cancelled', [], me, true)).toBeUndefined();
    expect(selfAssignOnStatusChange('completed', [], me, false)).toBeUndefined();
    expect(selfAssignOnStatusChange('completed', [], null, true)).toBeUndefined();
  });
});

describe('isMineOrUnassigned / classifyDoneByMe', () => {
  it('treats no-identity, unassigned, and assigned-to-me as mine', () => {
    expect(isMineOrUnassigned([other], null)).toBe(true);
    expect(isMineOrUnassigned([], me)).toBe(true);
    expect(isMineOrUnassigned([me], me)).toBe(true);
    expect(isMineOrUnassigned([me, other], me)).toBe(true);
  });

  it('treats a task assigned only to others as not mine', () => {
    expect(isMineOrUnassigned([other], me)).toBe(false);
  });

  it('classifyDoneByMe mirrors the ownership predicate', () => {
    expect(classifyDoneByMe([], me)).toBe('me');
    expect(classifyDoneByMe([me], me)).toBe('me');
    expect(classifyDoneByMe([other], me)).toBe('other');
    expect(classifyDoneByMe([other], null)).toBe('me');
  });
});

describe('task assignment → what a source can hold', () => {
  it('reads an undeclared adapter as "cannot assign"', () => {
    // The cautious default, mirroring `TaskCapabilities::default()`. Crediting
    // a silent manifest with assignment is how the picker came to offer a
    // choice the source could not keep.
    expect(taskAssignmentMode(undefined)).toBe('none');
    expect(taskAssignmentMode({ task_capabilities: undefined })).toBe('none');
    expect(
      taskAssignmentMode({
        task_capabilities: { subtasks: true } as never,
      }),
    ).toBe('none');
  });

  it('reads what the adapter declared', () => {
    expect(
      taskAssignmentMode({
        task_capabilities: { task_assignment: 'single' } as never,
      }),
    ).toBe('single');
  });

  it('trims a moved task to what the new list holds', () => {
    // Carry a two-assignee Vikunja task to a Todoist list: the form must lose
    // the second here, where it is visible, rather than on the wire.
    const anna = { id: 'u1', name: 'Anna', email: null } as never;
    const bernd = { id: 'u2', name: 'Bernd', email: null } as never;
    expect(clampAssignees('single', [anna, bernd])).toEqual([anna]);
    expect(clampAssignees('multiple', [anna, bernd])).toEqual([anna, bernd]);
  });

  it('never empties a list it merely cannot show', () => {
    // `none` hides the picker; the task may still carry assignees another
    // client wrote, and clearing them on open would destroy what the user was
    // never shown.
    const anna = { id: 'u1', name: 'Anna', email: null } as never;
    expect(clampAssignees('none', [anna])).toEqual([anna]);
  });
});
