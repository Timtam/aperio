import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Task } from '../api/types';
import { filterDeadlinePinTargets } from './deadlinePinTargets';

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'thing',
  description: null,
  status: 'open',
  priority: 'medium',
  effort: 'medium',
  scheduled_date: null,
  scheduled_time: null,
  deadline_date: null,
  deadline_time: null,
  recurrence: null,
  resurface_date: null,
  series_id: null,
  parent_id: null,
  section_id: null,
  color_label: null,
  reminders: [],
  assignees: [],
  sound: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  completed_at: null,
  etag: null,
};

describe('filterDeadlinePinTargets', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Pin "today" so all string comparisons stay deterministic.
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("picks open tasks whose deadline is today AND aren't already scheduled for today", () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-20' },
      {
        ...baseTask,
        id: 'b',
        deadline_date: '2026-05-20',
        status: 'in_progress',
      },
    ];
    expect(filterDeadlinePinTargets(tasks).map((t) => t.id).sort()).toEqual([
      'a',
      'b',
    ]);
  });

  it('skips tasks already scheduled for today (idempotent on re-launch)', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'a',
        deadline_date: '2026-05-20',
        scheduled_date: '2026-05-20',
      },
    ];
    expect(filterDeadlinePinTargets(tasks)).toEqual([]);
  });

  it('still pins when the task is scheduled for a different day (deadline wins)', () => {
    // Deadline-pin takes precedence over a stale future schedule — if
    // the deadline is today, the task lands on today regardless of
    // whatever scheduled day the user (or carry-over) wrote.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'a',
        deadline_date: '2026-05-20',
        scheduled_date: '2026-05-22',
      },
    ];
    expect(filterDeadlinePinTargets(tasks).map((t) => t.id)).toEqual(['a']);
  });

  it('skips past deadlines (DayStartReviewDialog territory)', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-19' },
    ];
    expect(filterDeadlinePinTargets(tasks)).toEqual([]);
  });

  it('skips future deadlines', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-21' },
    ];
    expect(filterDeadlinePinTargets(tasks)).toEqual([]);
  });

  it('skips completed and cancelled tasks', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'a',
        deadline_date: '2026-05-20',
        status: 'completed',
      },
      {
        ...baseTask,
        id: 'b',
        deadline_date: '2026-05-20',
        status: 'cancelled',
      },
    ];
    expect(filterDeadlinePinTargets(tasks)).toEqual([]);
  });

  it('skips tasks without a deadline_date', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', scheduled_date: '2026-05-19' },
    ];
    expect(filterDeadlinePinTargets(tasks)).toEqual([]);
  });
});
