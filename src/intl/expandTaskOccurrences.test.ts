import { describe, expect, it } from 'vitest';

import {
  TASK_RECURRENCE_DEFAULT,
  expandScheduledRecurringTasks,
  isRecurringProjection,
  makeOccurrenceId,
  recurringSeriesTaskId,
  toBackend,
  type TaskRecurrenceValue,
} from '@aperio/shared';

import type { Task } from '../api/types';

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

/** Backend recurrence JSON (what `Task.recurrence` holds) from a value patch. */
function rule(patch: Partial<TaskRecurrenceValue>): unknown {
  return toBackend({ ...TASK_RECURRENCE_DEFAULT, ...patch });
}

const ids = (ts: Task[]) => ts.map((t) => t.id);
const dates = (ts: Task[]) => ts.map((t) => t.scheduled_date);

describe('makeOccurrenceId / isRecurringProjection / recurringSeriesTaskId', () => {
  it('round-trips a base id through the occurrence encoding', () => {
    const occ = makeOccurrenceId('t1', '2026-05-02');
    expect(occ).toBe('t1 occ 2026-05-02');
    expect(isRecurringProjection(occ)).toBe(true);
    expect(isRecurringProjection('t1')).toBe(false);
    expect(recurringSeriesTaskId(occ)).toBe('t1');
    expect(recurringSeriesTaskId('t1')).toBe('t1');
  });

  it('accepts a task object as well as a bare id', () => {
    expect(isRecurringProjection({ id: 't1 occ 2026-05-02' })).toBe(true);
    expect(isRecurringProjection({ id: 't1' })).toBe(false);
    expect(recurringSeriesTaskId({ id: 't9 occ 2026-01-01' })).toBe('t9');
  });

  it('does not misclassify a real id that merely contains " occ "', () => {
    // Only an id ending in ` occ YYYY-MM-DD` is a projection — a real id that
    // happens to contain " occ " elsewhere (or without a date suffix) is left
    // interactive and unmodified.
    expect(isRecurringProjection('my occ notes')).toBe(false);
    expect(isRecurringProjection('occ 2026-05-02 leading')).toBe(false);
    expect(recurringSeriesTaskId('my occ notes')).toBe('my occ notes');
    // A base id that itself contains " occ " still round-trips: only the final
    // occurrence suffix is stripped.
    expect(recurringSeriesTaskId('my occ notes occ 2026-05-02')).toBe('my occ notes');
  });
});

describe('expandScheduledRecurringTasks — expansion', () => {
  it('daily rule projects every day in the window; base is the real task', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      scheduled_time: '09:00',
      recurrence: rule({ freq: 'DAILY' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-05');
    expect(dates(out)).toEqual([
      '2026-05-01',
      '2026-05-02',
      '2026-05-03',
      '2026-05-04',
      '2026-05-05',
    ]);
    // The occurrence on the task's own date is the REAL, interactive task.
    expect(out[0]).toBe(task);
    expect(isRecurringProjection(out[0])).toBe(false);
    // Every later day is a read-only projection routing back to the series.
    for (const p of out.slice(1)) {
      expect(isRecurringProjection(p)).toBe(true);
      expect(recurringSeriesTaskId(p)).toBe('t1');
      expect(p).not.toBe(task);
      expect(p.scheduled_time).toBe('09:00'); // time rides each occurrence
    }
    expect(ids(out)[1]).toBe('t1 occ 2026-05-02');
  });

  it('honours the interval (every 2 days)', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY', interval: 2 }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-07');
    expect(dates(out)).toEqual(['2026-05-01', '2026-05-03', '2026-05-05', '2026-05-07']);
  });

  it('weekly byDay projects only the listed weekdays', () => {
    // 2026-05-04 is a Monday.
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-04',
      recurrence: rule({ freq: 'WEEKLY', byDay: ['MO', 'WE', 'FR'] }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-04', '2026-05-15');
    expect(dates(out)).toEqual([
      '2026-05-04', // Mon
      '2026-05-06', // Wed
      '2026-05-08', // Fri
      '2026-05-11', // Mon
      '2026-05-13', // Wed
      '2026-05-15', // Fri
    ]);
  });

  it('monthly with day-of-month clamps to short months', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-01-31',
      recurrence: rule({ freq: 'MONTHLY', dayOfMonth: 31 }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-01-31', '2026-04-30');
    expect(dates(out)).toEqual([
      '2026-01-31',
      '2026-02-28', // clamped
      '2026-03-31', // back to 31
      '2026-04-30', // clamped
    ]);
  });

  it('yearly clamps a Feb-29 anchor on non-leap years', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2024-02-29',
      recurrence: rule({ freq: 'YEARLY' }),
    };
    const out = expandScheduledRecurringTasks([task], '2024-02-29', '2026-12-31');
    expect(dates(out)).toEqual(['2024-02-29', '2025-02-28', '2026-02-28']);
  });

  it('fixed dates drive a seasonal (yearless) schedule', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-04-01',
      recurrence: rule({
        freq: 'YEARLY',
        fixedDates: [
          { month: 4, day: 1 },
          { month: 10, day: 1 },
        ],
      }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-04-01', '2027-12-31');
    expect(dates(out)).toEqual([
      '2026-04-01',
      '2026-10-01',
      '2027-04-01',
      '2027-10-01',
    ]);
  });

  it('stops at the UNTIL bound', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY', endMode: 'UNTIL', until: '2026-05-03' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-10');
    expect(dates(out)).toEqual(['2026-05-01', '2026-05-02', '2026-05-03']);
  });

  it('only projects inside the window (base before the window → all projections)', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-03', '2026-05-05');
    expect(dates(out)).toEqual(['2026-05-03', '2026-05-04', '2026-05-05']);
    // None is the base date, so every one is a read-only projection.
    for (const p of out) {
      expect(isRecurringProjection(p)).toBe(true);
      expect(recurringSeriesTaskId(p)).toBe('t1');
    }
  });

  it('projects a far-past daily base into the window (cap counts emissions, not the walk)', () => {
    // The stored scheduled_date is ~18 months before the window — an old,
    // never-completed daily task keeps its original date. The cap must bound
    // only EMITTED occurrences, so the walk still reaches the window; a
    // per-iteration cap used to exhaust before arriving and drop the task.
    const task: Task = {
      ...baseTask,
      scheduled_date: '2025-01-01',
      recurrence: rule({ freq: 'DAILY' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-07-06', '2026-07-08');
    expect(dates(out)).toEqual(['2026-07-06', '2026-07-07', '2026-07-08']);
    for (const p of out) {
      expect(isRecurringProjection(p)).toBe(true);
      expect(recurringSeriesTaskId(p)).toBe('t1');
    }
  });

  it('projects a far-past weekly-byDay base into the window', () => {
    // 2025-01-06 is a Monday; the series recurs Mon/Thu. The window a year and a
    // half later must still receive its Mon/Thu occurrences.
    const task: Task = {
      ...baseTask,
      scheduled_date: '2025-01-06',
      recurrence: rule({ freq: 'WEEKLY', byDay: ['MO', 'TH'] }),
    };
    // 2026-07-06 is a Monday; 2026-07-09 a Thursday.
    const out = expandScheduledRecurringTasks([task], '2026-07-06', '2026-07-10');
    expect(dates(out)).toEqual(['2026-07-06', '2026-07-09']);
  });

  it('emits nothing when an UNTIL bound ends before the window (and terminates)', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2025-01-01',
      recurrence: rule({ freq: 'DAILY', endMode: 'UNTIL', until: '2025-06-01' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-07-06', '2026-07-08');
    expect(out).toEqual([]);
  });

  it('caps runaway daily rules at maxPerTask', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2027-05-01', 5);
    expect(out).toHaveLength(5);
    expect(dates(out)).toEqual([
      '2026-05-01',
      '2026-05-02',
      '2026-05-03',
      '2026-05-04',
      '2026-05-05',
    ]);
  });
});

describe('expandScheduledRecurringTasks — pass-through (no expansion)', () => {
  it('leaves a non-recurring task untouched', () => {
    const task: Task = { ...baseTask, scheduled_date: '2026-05-01', recurrence: null };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-10');
    expect(out).toHaveLength(1);
    expect(out[0]).toBe(task);
  });

  it('does not expand a from-completion rule (future dates are unknowable)', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY', anchor: 'FROM_COMPLETION' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-10');
    expect(out).toHaveLength(1);
    expect(out[0]).toBe(task);
  });

  it('does not expand a backlog-placement rule (its next turn is undated)', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY', placement: 'BACKLOG' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-10');
    expect(out).toHaveLength(1);
    expect(out[0]).toBe(task);
  });

  it('does not expand a recurring task without a scheduled_date', () => {
    const task: Task = {
      ...baseTask,
      scheduled_date: null,
      deadline_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY' }),
    };
    const out = expandScheduledRecurringTasks([task], '2026-05-01', '2026-05-10');
    expect(out).toHaveLength(1);
    expect(out[0]).toBe(task);
  });

  it('passes non-expandable tasks through alongside expanded ones', () => {
    const recurring: Task = {
      ...baseTask,
      id: 'rec',
      scheduled_date: '2026-05-01',
      recurrence: rule({ freq: 'DAILY' }),
    };
    const plain: Task = { ...baseTask, id: 'plain', scheduled_date: '2026-05-02' };
    const out = expandScheduledRecurringTasks([recurring, plain], '2026-05-01', '2026-05-03');
    expect(ids(out)).toEqual([
      'rec',
      'rec occ 2026-05-02',
      'rec occ 2026-05-03',
      'plain',
    ]);
  });
});
