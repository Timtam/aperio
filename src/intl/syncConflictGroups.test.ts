import { describe, expect, it } from 'vitest';

import {
  conflictGroupKey,
  groupSyncConflicts,
  type GroupableConflict,
} from '@aperio/shared';

const c = (row_kind: string, row_id: string, field: string) => ({
  row_kind,
  row_id,
  field,
});

describe('groupSyncConflicts', () => {
  it('groups a multi-field task into ONE group', () => {
    const out = groupSyncConflicts([
      c('task', 't1', 'status'),
      c('task', 't1', 'scheduled_date'),
      c('task', 't1', 'completed_at'),
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].rowKind).toBe('task');
    expect(out[0].rowId).toBe('t1');
    expect(out[0].conflicts.map((x) => x.field)).toEqual([
      'status',
      'scheduled_date',
      'completed_at',
    ]);
  });

  it('keeps distinct rows apart, even same field / same kind', () => {
    const out = groupSyncConflicts([
      c('task', 't1', 'scheduled_date'),
      c('task', 't2', 'scheduled_date'),
      c('task', 't3', 'scheduled_date'),
    ]);
    expect(out.map((g) => g.rowId)).toEqual(['t1', 't2', 't3']);
    expect(out.every((g) => g.conflicts.length === 1)).toBe(true);
  });

  it('separates the same id across different kinds', () => {
    const out = groupSyncConflicts([
      c('task', 'x', 'title'),
      c('event', 'x', 'title'),
    ]);
    expect(out).toHaveLength(2);
    expect(out.map((g) => g.key)).toEqual(['task:x', 'event:x']);
  });

  it('preserves first-seen order of groups AND interleaves back into them', () => {
    const out = groupSyncConflicts([
      c('task', 'a', 'status'),
      c('task', 'b', 'status'),
      c('task', 'a', 'scheduled_date'),
    ]);
    expect(out.map((g) => g.rowId)).toEqual(['a', 'b']);
    expect(out[0].conflicts.map((x) => x.field)).toEqual([
      'status',
      'scheduled_date',
    ]);
  });

  it('key matches conflictGroupKey', () => {
    const item: GroupableConflict = { row_kind: 'task', row_id: 't1' };
    expect(conflictGroupKey(item)).toBe('task:t1');
    expect(groupSyncConflicts([c('task', 't1', 'status')])[0].key).toBe(
      'task:t1',
    );
  });

  it('handles an empty list', () => {
    expect(groupSyncConflicts([])).toEqual([]);
  });
});
