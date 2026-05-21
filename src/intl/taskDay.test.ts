import { describe, expect, it } from 'vitest';

import type { Task } from '../api/types';
import { filterTasksOnDay } from './taskDay';

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'thing',
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

describe('filterTasksOnDay', () => {
  const today = '2026-05-20';

  it('matches scheduled_date == day', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'sched', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'other', scheduled_date: '2026-05-21' },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-20', today).map((t) => t.id))
      .toEqual(['sched']);
  });

  it('matches deadline_type=on AND deadline_date == day', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'on-match',
        deadline_type: 'on',
        deadline_date: '2026-05-22',
      },
      {
        ...baseTask,
        id: 'on-other',
        deadline_type: 'on',
        deadline_date: '2026-05-21',
      },
    ];
    expect(
      filterTasksOnDay(tasks, '2026-05-22', today).map((t) => t.id),
    ).toEqual(['on-match']);
  });

  it('By-task surfaces on every day from today through deadline', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'by',
        deadline_type: 'by',
        deadline_date: '2026-05-22',
      },
    ];
    // today (5/20) — inside window → yes
    expect(filterTasksOnDay(tasks, '2026-05-20', today).map((t) => t.id))
      .toEqual(['by']);
    // 5/21 — inside → yes
    expect(filterTasksOnDay(tasks, '2026-05-21', today).map((t) => t.id))
      .toEqual(['by']);
    // 5/22 — deadline itself → yes
    expect(filterTasksOnDay(tasks, '2026-05-22', today).map((t) => t.id))
      .toEqual(['by']);
    // 5/23 — past deadline → no
    expect(filterTasksOnDay(tasks, '2026-05-23', today)).toEqual([]);
    // 5/19 — before today → no (don't backfill past days)
    expect(filterTasksOnDay(tasks, '2026-05-19', today)).toEqual([]);
  });

  it('excludes completed and cancelled tasks', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'done',
        scheduled_date: '2026-05-20',
        status: 'completed',
      },
      {
        ...baseTask,
        id: 'cancelled',
        scheduled_date: '2026-05-20',
        status: 'cancelled',
      },
      {
        ...baseTask,
        id: 'live',
        scheduled_date: '2026-05-20',
        status: 'open',
      },
    ];
    expect(
      filterTasksOnDay(tasks, '2026-05-20', today).map((t) => t.id),
    ).toEqual(['live']);
  });

  it('returns empty when nothing matches', () => {
    expect(
      filterTasksOnDay([baseTask], '2026-05-20', today),
    ).toEqual([]);
  });
});
