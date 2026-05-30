import { describe, expect, it } from 'vitest';

import type { Task } from '../api/types';
import {
  buildDeadlineBars,
  filterScheduledTasksOnDay,
  filterTasksOnDay,
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
  scheduled_date: null,
  scheduled_time: null,
  deadline_date: null,
  deadline_time: null,
  recurrence: null,
  parent_id: null,
  section_id: null,
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

  it('matches scheduled_date == day for the merged "on" semantic', () => {
    // Post-migration 0006 the legacy `deadline_type='on'` semantics live
    // in `scheduled_date` — the day the user committed to. The filter
    // still picks it up the same way.
    const tasks: Task[] = [
      { ...baseTask, id: 'on-match', scheduled_date: '2026-05-22' },
      { ...baseTask, id: 'on-other', scheduled_date: '2026-05-21' },
    ];
    expect(
      filterTasksOnDay(tasks, '2026-05-22', today).map((t) => t.id),
    ).toEqual(['on-match']);
  });

  it('deadline-task surfaces on every day from today through deadline', () => {
    // With the migration `deadline_type` is gone; any task with a
    // `deadline_date` carries "by"-style window semantics.
    const tasks: Task[] = [
      { ...baseTask, id: 'by', deadline_date: '2026-05-22' },
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

  it('excludes subtasks (tasks with parent_id set)', () => {
    // Subtasks are scoped to their parent — calendar surfaces only
    // render top-level rows, regardless of whether the child carries
    // its own scheduled_date. The parent is the SoR for "is this on
    // my plate this day"; the children live inside it.
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'parent',
        scheduled_date: '2026-05-20',
      },
      {
        ...baseTask,
        id: 'child',
        parent_id: 'parent',
        scheduled_date: '2026-05-20',
      },
    ];
    expect(
      filterTasksOnDay(tasks, '2026-05-20', today).map((t) => t.id),
    ).toEqual(['parent']);
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
  it('returns scheduled_time on the scheduled day (legacy "on" semantic)', () => {
    // What used to be `deadline_type='on' + deadline_time` is now
    // `scheduled_date + scheduled_time`.
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
    // The Plan + Soft-Deadline edge case the migration preserves: both
    // slots set, possibly on the same day. The schedule-time wins as
    // the more action-oriented marker for that day.
    const task: Task = {
      ...baseTask,
      scheduled_date: '2026-05-22',
      scheduled_time: '09:00:00',
      deadline_date: '2026-05-22',
      deadline_time: '17:00:00',
    };
    expect(taskTimeOnDay(task, '2026-05-22')).toBe('09:00:00');
  });

  it('returns null on intermediate days of a deadline window', () => {
    // Even with a deadline_time, on an intermediate day there is no
    // time we can honestly point at — the user only committed to that
    // minute on the deadline date.
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
    expect(timed.map((i) => (i.kind === 'event' ? i.event.id : i.task.id)))
      .toEqual(['morning', 'task-noon', 'afternoon']);
    expect(untimed).toEqual([]);
  });

  it('untimed tasks go into the second bucket, not into the timed lane', () => {
    const events = [{ id: 'meeting', start: '2026-05-22T09:00:00' }];
    const tasks: Task[] = [
      // Pure scheduled — no time of day.
      { ...baseTask, id: 'sched', scheduled_date: '2026-05-22' },
      // Deadline-task on an intermediate day, even with deadline_time set.
      {
        ...baseTask,
        id: 'by-window',
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

describe('filterScheduledTasksOnDay', () => {
  it('matches scheduled_date == day only — no deadline-window fallthrough', () => {
    // The whole point of this filter (used by WeekView's per-day
    // chips): a task that's surfaced only by `deadline_date` shouldn't
    // appear as a chip on every day of the window. That lives in the
    // separate deadline-header lane.
    const tasks: Task[] = [
      { ...baseTask, id: 'sched', scheduled_date: '2026-05-20' },
      {
        ...baseTask,
        id: 'deadline-only',
        deadline_date: '2026-05-22',
      },
    ];
    expect(
      filterScheduledTasksOnDay(tasks, '2026-05-20').map((t) => t.id),
    ).toEqual(['sched']);
    // No fallback contribution from `deadline-only` on day 22 either —
    // the task only shows up if its scheduled_date matches.
    expect(filterScheduledTasksOnDay(tasks, '2026-05-22')).toEqual([]);
  });

  it('still hides subtasks, cancelled, and (opt-out) completed', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', scheduled_date: '2026-05-20' },
      {
        ...baseTask,
        id: 'sub',
        scheduled_date: '2026-05-20',
        parent_id: 'a',
      },
      {
        ...baseTask,
        id: 'cancel',
        scheduled_date: '2026-05-20',
        status: 'cancelled',
      },
      {
        ...baseTask,
        id: 'done',
        scheduled_date: '2026-05-20',
        status: 'completed',
      },
    ];
    expect(
      filterScheduledTasksOnDay(tasks, '2026-05-20').map((t) => t.id),
    ).toEqual(['a']);
    expect(
      filterScheduledTasksOnDay(tasks, '2026-05-20', () => true).map(
        (t) => t.id,
      ),
    ).toEqual(['a', 'done']);
  });
});

describe('buildDeadlineBars', () => {
  // Mon - Sun, ISO week. Use 2026-05-18..24 so today=2026-05-20 lands
  // mid-week (Wednesday) — gives us "before today", "today", and
  // "after today" columns to work with.
  const weekKeys = [
    '2026-05-18',
    '2026-05-19',
    '2026-05-20',
    '2026-05-21',
    '2026-05-22',
    '2026-05-23',
    '2026-05-24',
  ];
  const today = '2026-05-20';

  it('spans from today to deadline when both lie inside the visible week', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-22' },
    ];
    const bars = buildDeadlineBars(tasks, weekKeys, today);
    expect(bars).toEqual([
      {
        task: tasks[0],
        startCol: 3, // 2026-05-20 → idx 2 → 1-based col 3
        endCol: 5, // 2026-05-22 → idx 4 → 1-based col 5
        lane: 0,
        continuesBefore: false,
        continuesAfter: false,
      },
    ]);
  });

  it('ends at the visible-week boundary when the deadline is later', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-27' },
    ];
    const bars = buildDeadlineBars(tasks, weekKeys, today);
    expect(bars[0].endCol).toBe(7); // Sun column
    expect(bars[0].continuesAfter).toBe(true);
  });

  it('drops bars whose deadline is in the past (handled elsewhere)', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-19' },
    ];
    expect(buildDeadlineBars(tasks, weekKeys, today)).toEqual([]);
  });

  it('starts at the week boundary when today is in a previous week (future-week view)', () => {
    // User navigated to a future week. Today is before this week,
    // so the bar covers the whole week — and the left edge gets a
    // chevron because the window started earlier.
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-22' },
    ];
    const futureToday = '2026-05-11';
    const bars = buildDeadlineBars(tasks, weekKeys, futureToday);
    expect(bars[0].startCol).toBe(1);
    expect(bars[0].endCol).toBe(5);
    expect(bars[0].continuesBefore).toBe(true);
    expect(bars[0].continuesAfter).toBe(false);
  });

  it('skips weeks entirely in the past', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-22' },
    ];
    const tomorrowKeys = ['2026-05-04', '2026-05-05', '2026-05-06'];
    expect(
      buildDeadlineBars(tasks, tomorrowKeys, '2026-05-20'),
    ).toEqual([]);
  });

  it('skips subtasks, cancelled, and (opt-out) completed tasks', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'a', deadline_date: '2026-05-22' },
      {
        ...baseTask,
        id: 'sub',
        deadline_date: '2026-05-22',
        parent_id: 'a',
      },
      {
        ...baseTask,
        id: 'cancel',
        deadline_date: '2026-05-22',
        status: 'cancelled',
      },
      {
        ...baseTask,
        id: 'done',
        deadline_date: '2026-05-22',
        status: 'completed',
      },
    ];
    expect(buildDeadlineBars(tasks, weekKeys, today).map((b) => b.task.id)).toEqual([
      'a',
    ]);
    // With the opt-in callback, completed comes back.
    expect(
      buildDeadlineBars(tasks, weekKeys, today, () => true).map(
        (b) => b.task.id,
      ),
    ).toEqual(['a', 'done']);
  });

  it('lane-packs overlapping bars without colliding', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'mon-wed', deadline_date: '2026-05-20' }, // Wed
      { ...baseTask, id: 'wed-fri', deadline_date: '2026-05-22' }, // Wed-Fri
      { ...baseTask, id: 'thu-sat', deadline_date: '2026-05-23' }, // Wed-Sat (because today=Wed)
    ];
    const bars = buildDeadlineBars(tasks, weekKeys, today);
    // All three overlap on Wed (today). They must land on distinct
    // lanes — 0, 1, 2 — so they stack rather than overwriting each
    // other.
    const lanes = bars.map((b) => b.lane).sort();
    expect(lanes).toEqual([0, 1, 2]);
  });

});
