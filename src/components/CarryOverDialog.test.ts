import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';

import type { Task } from '../api/types';
import { actionableDescendants, filterCarriedOver } from './CarryOverDialog';

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'something',
  description: null,
  status: 'open',
  priority: 'medium',
  scheduled_date: null,
  deadline_type: null,
  deadline_date: null,
  deadline_time: null,
  recurrence: null,
  parent_id: null,
  color_label: null,
  reminders: [],
  sound: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  completed_at: null,
  etag: null,
};

describe('filterCarriedOver', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Pin "today" at 2026-05-20 so all string comparisons stay
    // deterministic regardless of the host's clock.
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('picks open tasks with scheduled_date strictly before today', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', scheduled_date: '2026-05-19' },
      { ...baseTask, id: 'b', scheduled_date: '2026-05-18' },
    ];
    const result = filterCarriedOver(tasks);
    expect(result.map((t) => t.id)).toEqual(['a', 'b']);
  });

  it('ignores tasks scheduled for today', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', scheduled_date: '2026-05-20' },
    ];
    expect(filterCarriedOver(tasks)).toEqual([]);
  });

  it('ignores tasks scheduled in the future', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', scheduled_date: '2026-05-21' },
    ];
    expect(filterCarriedOver(tasks)).toEqual([]);
  });

  it('ignores backlog tasks (scheduled_date null)', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', scheduled_date: null },
    ];
    expect(filterCarriedOver(tasks)).toEqual([]);
  });

  it('ignores completed and cancelled tasks regardless of date', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'a',
        scheduled_date: '2026-05-15',
        status: 'completed',
      },
      {
        ...baseTask,
        id: 'b',
        scheduled_date: '2026-05-15',
        status: 'cancelled',
      },
    ];
    expect(filterCarriedOver(tasks)).toEqual([]);
  });

  it('picks in_progress tasks the same as open ones', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'a',
        scheduled_date: '2026-05-19',
        status: 'in_progress',
      },
    ];
    expect(filterCarriedOver(tasks).map((t) => t.id)).toEqual(['a']);
  });

  it('includes subtasks that have slipped on their own (cascade off)', () => {
    // Per-task filter — scheduled_date never cascades, so subtasks
    // appear in the list independently of their parents when the
    // user opted out of status-coupling.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'parent',
        scheduled_date: null,
      },
      {
        ...baseTask,
        id: 'child',
        parent_id: 'parent',
        scheduled_date: '2026-05-19',
      },
    ];
    expect(filterCarriedOver(tasks).map((t) => t.id)).toEqual(['child']);
  });
});

describe('filterCarriedOver — cascade-coupling honoured', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('hides slipped subtasks whose parent is also slipped', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'parent', scheduled_date: '2026-05-19' },
      {
        ...baseTask,
        id: 'child',
        parent_id: 'parent',
        scheduled_date: '2026-05-19',
      },
    ];
    expect(
      filterCarriedOver(tasks, { cascadeEnabled: true }).map((t) => t.id),
    ).toEqual(['parent']);
  });

  it('keeps a slipped subtask whose parent is not slipped', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'parent', scheduled_date: null },
      {
        ...baseTask,
        id: 'child',
        parent_id: 'parent',
        scheduled_date: '2026-05-19',
      },
    ];
    expect(
      filterCarriedOver(tasks, { cascadeEnabled: true }).map((t) => t.id),
    ).toEqual(['child']);
  });

  it('hides slipped grandchildren when the grandparent is also slipped', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'g', scheduled_date: '2026-05-19' },
      {
        ...baseTask,
        id: 'p',
        parent_id: 'g',
        scheduled_date: '2026-05-19',
      },
      {
        ...baseTask,
        id: 'c',
        parent_id: 'p',
        scheduled_date: '2026-05-19',
      },
    ];
    expect(
      filterCarriedOver(tasks, { cascadeEnabled: true }).map((t) => t.id),
    ).toEqual(['g']);
  });

  it('cascade-off matches the parameterless behaviour', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'parent', scheduled_date: '2026-05-19' },
      {
        ...baseTask,
        id: 'child',
        parent_id: 'parent',
        scheduled_date: '2026-05-19',
      },
    ];
    expect(
      filterCarriedOver(tasks, { cascadeEnabled: false }).map((t) => t.id),
    ).toEqual(filterCarriedOver(tasks).map((t) => t.id));
  });
});

describe('actionableDescendants', () => {
  it('collects open and in_progress descendants recursively', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'a', parent_id: 'p', status: 'open' },
      { ...baseTask, id: 'b', parent_id: 'p', status: 'in_progress' },
      { ...baseTask, id: 'c', parent_id: 'a', status: 'open' },
    ];
    expect(actionableDescendants('p', tasks).map((t) => t.id).sort()).toEqual(
      ['a', 'b', 'c'].sort(),
    );
  });

  it('skips completed and cancelled descendants', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'a', parent_id: 'p', status: 'completed' },
      { ...baseTask, id: 'b', parent_id: 'p', status: 'cancelled' },
    ];
    expect(actionableDescendants('p', tasks)).toEqual([]);
  });

  it('still traverses through a settled middle node to reach open leaves', () => {
    // A completed middle child shouldn't itself be a cascade target,
    // but its open grandchildren still should be — we don't want a
    // stale completion in the middle of the tree to orphan live
    // descendants.
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'm', parent_id: 'p', status: 'completed' },
      { ...baseTask, id: 'leaf', parent_id: 'm', status: 'open' },
    ];
    expect(actionableDescendants('p', tasks).map((t) => t.id)).toEqual(['leaf']);
  });
});
