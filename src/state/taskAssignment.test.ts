import { describe, expect, it } from 'vitest';

import type { TaskUser } from '../api/types';
import {
  classifyDoneByMe,
  isMineOrUnassigned,
  selfAssignOnStatusChange,
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
