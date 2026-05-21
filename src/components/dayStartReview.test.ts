import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';

import type { Task } from '../api/types';
import {
  actionableDescendants,
  filterCarriedOver,
  filterOverdue,
  isDayStartReviewSnoozed,
  snoozeDayStartReview,
} from './dayStartReview';

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'something',
  description: null,
  status: 'open',
  priority: 'medium',
  scheduled_date: null,
  scheduled_time: null,
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

describe('filterOverdue', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('picks tasks with a deadline strictly before today', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'past', deadline_date: '2026-05-19' },
      { ...baseTask, id: 'today', deadline_date: '2026-05-20' },
      { ...baseTask, id: 'future', deadline_date: '2026-05-25' },
    ];
    const overdue = filterOverdue(tasks);
    expect(overdue.map((t) => t.id)).toEqual(['past']);
  });

  it('ignores tasks without a deadline (scheduled_date alone is not a missed commitment)', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'scheduled-only',
        deadline_date: null,
        scheduled_date: '2026-04-01',
      },
    ];
    expect(filterOverdue(tasks)).toHaveLength(0);
  });

  it('ignores completed and cancelled tasks regardless of deadline', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'done',
        deadline_date: '2026-05-19',
        status: 'completed',
      },
      {
        ...baseTask,
        id: 'cancelled',
        deadline_date: '2026-05-19',
        status: 'cancelled',
      },
      {
        ...baseTask,
        id: 'inprogress',
        deadline_date: '2026-05-19',
        status: 'in_progress',
      },
    ];
    // in_progress is NOT terminal — still a missed commitment.
    expect(filterOverdue(tasks).map((t) => t.id)).toEqual(['inprogress']);
  });
});

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

  it('drops rows that are also overdue (deadline takes priority)', () => {
    // The dialog renders both sections from the same task list; if a
    // task satisfies both filters the deadline half wins and the
    // carry-over half skips it, so the user only handles the row
    // once.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'both',
        scheduled_date: '2026-05-18',
        deadline_date: '2026-05-19',
      },
      {
        ...baseTask,
        id: 'sched-only',
        scheduled_date: '2026-05-18',
      },
    ];
    expect(filterOverdue(tasks).map((t) => t.id)).toEqual(['both']);
    expect(filterCarriedOver(tasks).map((t) => t.id)).toEqual(['sched-only']);
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

describe('snoozeDayStartReview / isDayStartReviewSnoozed', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
    localStorage.clear();
  });
  afterEach(() => {
    vi.useRealTimers();
    localStorage.clear();
  });

  it('is not snoozed by default', () => {
    expect(isDayStartReviewSnoozed()).toBe(false);
  });

  it('snooze for 4 hours blocks for 4 hours, releases on hour 5', () => {
    snoozeDayStartReview(4);
    expect(isDayStartReviewSnoozed()).toBe(true);

    // Three hours later — still snoozed.
    vi.setSystemTime(new Date(2026, 4, 20, 15, 0, 0));
    expect(isDayStartReviewSnoozed()).toBe(true);

    // Five hours later — released.
    vi.setSystemTime(new Date(2026, 4, 20, 17, 0, 1));
    expect(isDayStartReviewSnoozed()).toBe(false);
  });

  it('treats a corrupted snooze value as not snoozed', () => {
    localStorage.setItem('aperio.dayStartReview.snoozeUntil', 'not-a-number');
    expect(isDayStartReviewSnoozed()).toBe(false);
  });

  it('honours a still-live legacy missed-tasks snooze key', () => {
    // Upgrade case: a user on the old build set a 4-hour snooze on
    // the missed-tasks dialog and then updates. Their snooze should
    // still hold rather than the unified gate firing immediately on
    // app start.
    const until = Date.now() + 60 * 60 * 1000;
    localStorage.setItem('aperio.missedTasks.snoozeUntil', String(until));
    expect(isDayStartReviewSnoozed()).toBe(true);
  });

  it('honours a still-live legacy carry-over snooze key', () => {
    const until = Date.now() + 60 * 60 * 1000;
    localStorage.setItem('aperio.carryOver.snoozeUntil', String(until));
    expect(isDayStartReviewSnoozed()).toBe(true);
  });
});
