import { describe, expect, it } from 'vitest';

import type { Task } from '../api/types';
import { filterTasksOnDay, mergeDayItems, taskTimeOnDay } from './taskDay';

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

  it('keeps completed tasks when the per-list opt-in is true', () => {
    // Sidebar context menu: "Erledigte Aufgaben in Kalenderansicht
    // anzeigen". The pref is per-list, so completed rows on lists
    // the user opted into stay visible, while completed rows on
    // other lists still vanish.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'done-shown',
        list_id: 'list-A',
        scheduled_date: '2026-05-20',
        status: 'completed',
      },
      {
        ...baseTask,
        id: 'done-hidden',
        list_id: 'list-B',
        scheduled_date: '2026-05-20',
        status: 'completed',
      },
      {
        ...baseTask,
        id: 'live',
        list_id: 'list-B',
        scheduled_date: '2026-05-20',
        status: 'open',
      },
    ];
    const visible = (listId: string) => listId === 'list-A';
    expect(
      filterTasksOnDay(tasks, '2026-05-20', today, visible).map((t) => t.id),
    ).toEqual(['done-shown', 'live']);
  });

  it('still hides cancelled tasks even when the opt-in is true', () => {
    // Cancelled is a distinct status — the user explicitly walked
    // away from the row, so it doesn't belong on the calendar even
    // when the "show completed" flag is on.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'cancel',
        list_id: 'list-A',
        scheduled_date: '2026-05-20',
        status: 'cancelled',
      },
    ];
    const visible = () => true;
    expect(
      filterTasksOnDay(tasks, '2026-05-20', today, visible),
    ).toEqual([]);
  });

  it('returns empty when nothing matches', () => {
    expect(
      filterTasksOnDay([baseTask], '2026-05-20', today),
    ).toEqual([]);
  });
});

describe('taskTimeOnDay', () => {
  it('returns the deadline_time on the deadline day', () => {
    const task: Task = {
      ...baseTask,
      deadline_type: 'on',
      deadline_date: '2026-05-22',
      deadline_time: '14:30:00',
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBe('14:30:00');
  });

  it('returns null on intermediate days of a By-window', () => {
    // Even when the task does carry a deadline_time, on an
    // intermediate day there is no time we can honestly point at —
    // the user only committed to that minute on the deadline date.
    const task: Task = {
      ...baseTask,
      deadline_type: 'by',
      deadline_date: '2026-05-22',
      deadline_time: '14:30:00',
    };
    expect(taskTimeOnDay(task, '2026-05-21')).toBeNull();
  });

  it('returns null when the user did not pick a time', () => {
    const task: Task = {
      ...baseTask,
      deadline_type: 'on',
      deadline_date: '2026-05-22',
      deadline_time: null,
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBeNull();
  });
});

describe('mergeDayItems', () => {
  const eventTime = (e: { start: string }) => new Date(e.start).getTime();

  it('interleaves timed tasks with events sorted by time', () => {
    // The exact bug the user reported: a 14:00 task lands above a
    // 15:00 event, not at the bottom of the day cell.
    const events = [
      { id: 'morning', start: '2026-05-22T09:00:00' },
      { id: 'afternoon', start: '2026-05-22T15:00:00' },
    ];
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'task-noon',
        deadline_type: 'on',
        deadline_date: '2026-05-22',
        deadline_time: '14:00:00',
      },
    ];
    const { timed, untimed } = mergeDayItems(
      events,
      tasks,
      '2026-05-22',
      eventTime,
    );
    expect(timed.map((i) => (i.kind === 'event' ? i.event.id : i.task.id)))
      .toEqual(['morning', 'task-noon', 'afternoon']);
    expect(untimed).toEqual([]);
  });

  it('untimed tasks go into the second bucket, not into the timed lane', () => {
    const events = [{ id: 'meeting', start: '2026-05-22T09:00:00' }];
    const tasks: Task[] = [
      // Pure scheduled — no time of day.
      { ...baseTask, id: 'sched', scheduled_date: '2026-05-22' },
      // By-task intermediate day, even with deadline_time set.
      {
        ...baseTask,
        id: 'by-window',
        deadline_type: 'by',
        deadline_date: '2026-05-22',
        deadline_time: '14:00:00',
      },
    ];
    const { timed, untimed } = mergeDayItems(
      events,
      tasks,
      '2026-05-21',
      eventTime,
    );
    expect(timed.map((i) => (i.kind === 'event' ? i.event.id : i.task.id)))
      .toEqual(['meeting']);
    expect(untimed.map((t) => t.id)).toEqual(['sched', 'by-window']);
  });
});
