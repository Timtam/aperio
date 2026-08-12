import { describe, expect, it } from 'vitest';

import type { Task } from '../api/types';
import {
  filterTasksOnDay,
  isDeadlineChip,
  mergeDayItems,
  taskTimeOnDay,
} from './taskDay';

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
  deadline_reminder_days: null,
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

describe('filterTasksOnDay', () => {
  it('matches scheduled_date == day', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'sched', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'other', scheduled_date: '2026-05-21' },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-20').map((t) => t.id)).toEqual([
      'sched',
    ]);
  });

  it('matches scheduled_date == day for the merged "on" semantic', () => {
    // Post-migration 0006 the legacy `deadline_type='on'` semantics live
    // in `scheduled_date` — the day the user committed to.
    const tasks: Task[] = [
      { ...baseTask, id: 'on-match', scheduled_date: '2026-05-22' },
      { ...baseTask, id: 'on-other', scheduled_date: '2026-05-21' },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-22').map((t) => t.id)).toEqual([
      'on-match',
    ]);
  });

  it('surfaces a deadline task only on its deadline day (point, not span)', () => {
    // A `deadline_date` puts the task on its deadline day as a single
    // point marker — NOT on every day from today until then (that span
    // cluttered the planner for far-future deadlines).
    const tasks: Task[] = [
      { ...baseTask, id: 'by', deadline_date: '2026-05-22' },
    ];
    // deadline day → yes
    expect(filterTasksOnDay(tasks, '2026-05-22').map((t) => t.id)).toEqual([
      'by',
    ]);
    // day before the deadline → no (no window)
    expect(filterTasksOnDay(tasks, '2026-05-21')).toEqual([]);
    // day after the deadline → no
    expect(filterTasksOnDay(tasks, '2026-05-23')).toEqual([]);
  });

  it('surfaces a scheduled+deadline task ONLY on its scheduled day', () => {
    // Scheduled Wed, due Fri → a single work chip on Wed (which announces
    // the deadline). It does NOT also appear on the deadline day: the plan
    // is its home, so the deadline-day duplicate is suppressed.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'both',
        scheduled_date: '2026-05-20',
        deadline_date: '2026-05-22',
      },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-20').map((t) => t.id)).toEqual([
      'both',
    ]);
    expect(filterTasksOnDay(tasks, '2026-05-21')).toEqual([]);
    // Suppressed on the deadline day — it's already shown on the plan day.
    expect(filterTasksOnDay(tasks, '2026-05-22')).toEqual([]);
  });

  it('surfaces a scheduled==deadline task once on that day', () => {
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'same',
        scheduled_date: '2026-05-22',
        deadline_date: '2026-05-22',
      },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-22').map((t) => t.id)).toEqual([
      'same',
    ]);
  });

  it('shows a subtask only when it carries its own date', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'parent', scheduled_date: '2026-05-20' },
      {
        ...baseTask,
        id: 'dated-child',
        parent_id: 'parent',
        scheduled_date: '2026-05-20',
      },
      {
        ...baseTask,
        id: 'undated-child',
        parent_id: 'parent',
        scheduled_date: null,
        deadline_date: null,
      },
    ];
    // The dated subtask surfaces as its own chip; the undated one stays hidden
    // (it travels with its parent).
    expect(filterTasksOnDay(tasks, '2026-05-20').map((t) => t.id)).toEqual([
      'parent',
      'dated-child',
    ]);
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
      { ...baseTask, id: 'live', scheduled_date: '2026-05-20', status: 'open' },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-20').map((t) => t.id)).toEqual([
      'live',
    ]);
  });

  it('keeps completed tasks when the per-list opt-in is true', () => {
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
      filterTasksOnDay(tasks, '2026-05-20', visible).map((t) => t.id),
    ).toEqual(['done-shown', 'live']);
  });

  it('still hides cancelled tasks even when the opt-in is true', () => {
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
    expect(filterTasksOnDay(tasks, '2026-05-20', visible)).toEqual([]);
  });

  it("orders a day's tasks like the task list (priority band, natural A→Z)", () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'b10', title: 'Aufgabe 10', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'b2', title: 'Aufgabe 2', scheduled_date: '2026-05-20' },
      // High priority floats above the band regardless of title.
      {
        ...baseTask,
        id: 'hi',
        title: 'Zuletzt im Alphabet',
        priority: 'high',
        scheduled_date: '2026-05-20',
      },
      { ...baseTask, id: 'lo', title: 'Anfang', priority: 'low', scheduled_date: '2026-05-20' },
    ];
    expect(filterTasksOnDay(tasks, '2026-05-20').map((t) => t.id)).toEqual([
      'hi',
      'b2',
      'b10',
      'lo',
    ]);
  });

  it('returns empty when nothing matches', () => {
    expect(filterTasksOnDay([baseTask], '2026-05-20')).toEqual([]);
  });
});

describe('isDeadlineChip', () => {
  it('is true on the deadline day when not also scheduled there', () => {
    expect(
      isDeadlineChip({ ...baseTask, deadline_date: '2026-05-22' }, '2026-05-22'),
    ).toBe(true);
  });

  it('is false on a non-deadline day', () => {
    expect(
      isDeadlineChip({ ...baseTask, deadline_date: '2026-05-22' }, '2026-05-21'),
    ).toBe(false);
  });

  it('is false when the task has no deadline', () => {
    expect(
      isDeadlineChip(
        { ...baseTask, scheduled_date: '2026-05-22' },
        '2026-05-22',
      ),
    ).toBe(false);
  });

  it('is false when scheduled AND due the same day (schedule wins)', () => {
    expect(
      isDeadlineChip(
        {
          ...baseTask,
          scheduled_date: '2026-05-22',
          deadline_date: '2026-05-22',
        },
        '2026-05-22',
      ),
    ).toBe(false);
  });

  it('is true on the deadline day even when scheduled on a different day', () => {
    expect(
      isDeadlineChip(
        {
          ...baseTask,
          scheduled_date: '2026-05-20',
          deadline_date: '2026-05-22',
        },
        '2026-05-22',
      ),
    ).toBe(true);
  });
});

describe('taskTimeOnDay', () => {
  it('returns scheduled_time on the scheduled day (legacy "on" semantic)', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-22',
      scheduled_time: '14:30:00',
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBe('14:30:00');
  });

  it('returns deadline_time on the deadline day when only a deadline is set', () => {
    const task: Task = {
      ...baseTask,
      deadline_date: '2026-05-22',
      deadline_time: '17:00:00',
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBe('17:00:00');
  });

  it('prefers scheduled_time when both could apply on the same day', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-22',
      scheduled_time: '09:00:00',
      deadline_date: '2026-05-22',
      deadline_time: '17:00:00',
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBe('09:00:00');
  });

  it('returns null on a day that is neither the scheduled nor deadline day', () => {
    const task: Task = {
      ...baseTask,
      deadline_date: '2026-05-22',
      deadline_time: '14:30:00',
    };
    expect(taskTimeOnDay(task, '2026-05-21')).toBeNull();
  });

  it('returns null when the user did not pick a time', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-22',
      scheduled_time: null,
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBeNull();
  });
});

describe('mergeDayItems', () => {
  const eventTime = (e: { start: string }) => new Date(e.start).getTime();

  it('interleaves timed tasks with events sorted by time', () => {
    const events = [
      { id: 'morning', start: '2026-05-22T09:00:00' },
      { id: 'afternoon', start: '2026-05-22T15:00:00' },
    ];
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'task-noon',
        scheduled_date: '2026-05-22',
        scheduled_time: '14:00:00',
      },
    ];
    const { timed, untimed } = mergeDayItems(
      events,
      tasks,
      '2026-05-22',
      eventTime,
    );
    expect(
      timed.map((i) => (i.kind === 'event' ? i.event.id : i.task.id)),
    ).toEqual(['morning', 'task-noon', 'afternoon']);
    expect(untimed).toEqual([]);
  });

  it('untimed tasks go into the second bucket, not into the timed lane', () => {
    const events = [{ id: 'meeting', start: '2026-05-22T09:00:00' }];
    const tasks: Task[] = [
      // Pure scheduled — no time of day.
      { ...baseTask, id: 'sched', scheduled_date: '2026-05-21' },
      // Deadline task whose deadline is a different day → no time here.
      {
        ...baseTask,
        id: 'by-other-day',
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
    expect(
      timed.map((i) => (i.kind === 'event' ? i.event.id : i.task.id)),
    ).toEqual(['meeting']);
    expect(untimed.map((t) => t.id)).toEqual(['sched', 'by-other-day']);
  });
});

describe('filterTasksOnDay — where a finished task sits', () => {
  const shown = () => true;

  it('moves a completed task with NO plan day onto its completion day', () => {
    // Reported from Vikunja: a deadline on the 6th, never scheduled, ticked off
    // on the 5th. It showed as done in the task list and then appeared in the
    // calendar on the 6th — a tick on a day for work that was already over,
    // while the 5th looked like nothing had happened.
    const done: Task = {
      ...baseTask,
      id: 'abgehakt',
      deadline_date: '2026-08-06',
      status: 'completed',
      completed_at: new Date(2026, 7, 5, 18, 0).toISOString(),
    };
    expect(filterTasksOnDay([done], '2026-08-05', shown).map((t) => t.id)).toEqual([
      'abgehakt',
    ]);
    expect(filterTasksOnDay([done], '2026-08-06', shown)).toEqual([]);
  });

  it('reads the completion instant in LOCAL time', () => {
    // Late in the evening at a positive offset, the UTC date is already
    // tomorrow. Taking the date off the raw string would file the task a day
    // late — the same trap that once cost the recurrence resurface a day.
    const lateEvening = new Date(2026, 7, 5, 23, 30);
    const done: Task = {
      ...baseTask,
      id: 'spaet',
      deadline_date: '2026-08-01',
      status: 'completed',
      completed_at: lateEvening.toISOString(),
    };
    expect(filterTasksOnDay([done], '2026-08-05', shown).map((t) => t.id)).toEqual([
      'spaet',
    ]);
  });

  it('falls back to the deadline day when no completion instant was recorded', () => {
    // An adapter that keeps no timestamp must not lose the task altogether.
    const done: Task = {
      ...baseTask,
      id: 'ohne-zeitstempel',
      deadline_date: '2026-08-04',
      status: 'completed',
      completed_at: null,
    };
    expect(filterTasksOnDay([done], '2026-08-04', shown).map((t) => t.id)).toEqual([
      'ohne-zeitstempel',
    ]);
  });

  it('keeps a completed task on the day it was PLANNED for', () => {
    // Reported: a daily "take pills", forgotten one day and ticked off the
    // next. Filing it under the day of the tick emptied yesterday and put the
    // finished dose next to the one still to take — a log that reads as a
    // skipped day followed by a doubled one.
    const yesterdaysDose: Task = {
      ...baseTask,
      id: 'tabletten-gestern',
      scheduled_date: '2026-08-11',
      status: 'completed',
      completed_at: new Date(2026, 7, 12, 8, 30).toISOString(),
    };
    const todaysDose: Task = {
      ...baseTask,
      id: 'tabletten-heute',
      scheduled_date: '2026-08-12',
    };
    const tasks = [yesterdaysDose, todaysDose];
    expect(filterTasksOnDay(tasks, '2026-08-11', shown).map((t) => t.id)).toEqual([
      'tabletten-gestern',
    ]);
    expect(filterTasksOnDay(tasks, '2026-08-12', shown).map((t) => t.id)).toEqual([
      'tabletten-heute',
    ]);
  });

  it('follows the editor when the date of a finished task is changed', () => {
    // Moving a finished task used to change nothing on the calendar: the
    // completion day had already decided where it sat.
    const done: Task = {
      ...baseTask,
      id: 'verschoben',
      scheduled_date: '2026-08-11',
      status: 'completed',
      completed_at: new Date(2026, 7, 12, 8, 30).toISOString(),
    };
    const moved: Task = { ...done, scheduled_date: '2026-08-09' };
    expect(filterTasksOnDay([moved], '2026-08-09', shown).map((t) => t.id)).toEqual([
      'verschoben',
    ]);
    expect(filterTasksOnDay([moved], '2026-08-11', shown)).toEqual([]);
  });
});
