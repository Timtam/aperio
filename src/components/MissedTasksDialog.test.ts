import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';

import type { Task } from '../api/types';
import {
  filterOverdue,
  isCurrentlySnoozed,
  snoozeUntilNextHour,
} from './MissedTasksDialog';

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'something',
  description: null,
  status: 'open',
  priority: 'medium',
  scheduled_date: null,
  deadline_type: 'on',
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

describe('snoozeUntilNextHour / isCurrentlySnoozed', () => {
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
    expect(isCurrentlySnoozed()).toBe(false);
  });

  it('snooze for 4 hours blocks for 4 hours, releases on hour 5', () => {
    snoozeUntilNextHour(4);
    expect(isCurrentlySnoozed()).toBe(true);

    // Three hours later — still snoozed.
    vi.setSystemTime(new Date(2026, 4, 20, 15, 0, 0));
    expect(isCurrentlySnoozed()).toBe(true);

    // Five hours later — released.
    vi.setSystemTime(new Date(2026, 4, 20, 17, 0, 1));
    expect(isCurrentlySnoozed()).toBe(false);
  });

  it('treats a corrupted snooze value as not snoozed', () => {
    localStorage.setItem('aperio.missedTasks.snoozeUntil', 'not-a-number');
    expect(isCurrentlySnoozed()).toBe(false);
  });
});
