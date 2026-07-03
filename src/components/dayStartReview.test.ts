import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';

import type { Task, TaskUser } from '../api/types';
import {
  actionableDescendants,
  filterCarriedOver,
  filterOverdue,
  isDayStartReviewSnoozed,
  snoozeDayStartReview,
} from './dayStartReview';
import {
  buildReminderGroups,
  daysUntilDeadline,
  filterDeadlineArrived,
  filterDeadlineCountdown,
  filterDeadlinePinTargets,
  filterUntimedToday,
  hasActionableDescendants,
  reminderCount,
} from '@aperio/shared';

const allRemindersOn = {
  remindUntimedToday: true,
  remindDeadlineArrived: true,
  remindDeadlineCountdown: true,
  deadlineCountdownDays: 3,
};

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'something',
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

describe('project-parent suppression (parents with open subtasks)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0)); // 2026-05-20
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('filterOverdue suppresses a deadline parent while it has open subtasks', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'paper', deadline_date: '2026-05-18' }, // past deadline
      {
        ...baseTask,
        id: 'sub-a',
        parent_id: 'paper',
        status: 'open',
        scheduled_date: '2026-05-19',
      },
      { ...baseTask, id: 'sub-b', parent_id: 'paper', status: 'completed' },
    ];
    // The parent is a project (sub-a still open) → not nagged about here; the
    // subtasks are the surfaced units.
    expect(filterOverdue(tasks).map((t) => t.id)).toEqual([]);
  });

  it('filterOverdue surfaces the parent again once all subtasks are settled', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'paper', deadline_date: '2026-05-18' },
      { ...baseTask, id: 'sub-a', parent_id: 'paper', status: 'completed' },
      { ...baseTask, id: 'sub-b', parent_id: 'paper', status: 'cancelled' },
    ];
    // No open subtasks left → the parent returns so its deadline can be closed.
    expect(filterOverdue(tasks).map((t) => t.id)).toEqual(['paper']);
  });

  it('filterDeadlinePinTargets never pins a project parent on its deadline day', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'paper', deadline_date: '2026-05-20' }, // deadline today
      {
        ...baseTask,
        id: 'sub-a',
        parent_id: 'paper',
        status: 'in_progress',
      },
    ];
    expect(filterDeadlinePinTargets(tasks).map((t) => t.id)).toEqual([]);
  });

  it('hasActionableDescendants walks nested subtrees and early-exits', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'root' },
      { ...baseTask, id: 'mid', parent_id: 'root', status: 'completed' },
      { ...baseTask, id: 'leaf', parent_id: 'mid', status: 'open' },
    ];
    // An open grandchild under a completed child still counts.
    expect(hasActionableDescendants('root', tasks)).toBe(true);
    expect(hasActionableDescendants('mid', tasks)).toBe(true);
    expect(hasActionableDescendants('leaf', tasks)).toBe(false);
  });
});

describe('day-start ownership filter (meFor)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  const me: TaskUser = { id: 'u1', name: 'Me', email: null };
  const other: TaskUser = { id: 'u2', name: 'Other', email: null };
  const meFor = () => me;

  it('filterOverdue keeps mine + unassigned, drops a colleague task', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'mine', deadline_date: '2026-05-19', assignees: [me] },
      { ...baseTask, id: 'unassigned', deadline_date: '2026-05-19', assignees: [] },
      { ...baseTask, id: 'theirs', deadline_date: '2026-05-19', assignees: [other] },
    ];
    expect(filterOverdue(tasks, meFor).map((t) => t.id)).toEqual(['mine', 'unassigned']);
    // Without meFor every overdue task is returned (back-compat).
    expect(filterOverdue(tasks)).toHaveLength(3);
  });

  it('filterCarriedOver drops a colleague slipped task', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'mine', scheduled_date: '2026-05-18', assignees: [me] },
      { ...baseTask, id: 'theirs', scheduled_date: '2026-05-18', assignees: [other] },
    ];
    expect(filterCarriedOver(tasks, { meFor }).map((t) => t.id)).toEqual(['mine']);
  });

  it('no identity (meFor → null) keeps everything', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'theirs', deadline_date: '2026-05-19', assignees: [other] },
    ];
    expect(filterOverdue(tasks, () => null).map((t) => t.id)).toEqual(['theirs']);
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
      filterCarriedOver(tasks, { cascadeEnabledFor: () => true }).map((t) => t.id),
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
      filterCarriedOver(tasks, { cascadeEnabledFor: () => true }).map((t) => t.id),
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
      filterCarriedOver(tasks, { cascadeEnabledFor: () => true }).map((t) => t.id),
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
    // `cascadeEnabledFor: () => false` and "no callback at all" must
    // yield identical results — both mean "no cascade".
    expect(
      filterCarriedOver(tasks, { cascadeEnabledFor: () => false }).map((t) => t.id),
    ).toEqual(filterCarriedOver(tasks).map((t) => t.id));
  });

  it('honours per-list cascade callback for mixed lists', () => {
    // Two parent/child pairs in different lists. The first list has
    // cascade on (subtask should hide); the second has cascade off
    // (both rows surface independently). Mirrors the user-facing
    // case of "Work cascades, Hobby doesn't".
    const tasks: Task[] = [
      {
        ...baseTask,
        id: 'work-parent',
        list_id: 'work',
        scheduled_date: '2026-05-19',
      },
      {
        ...baseTask,
        id: 'work-child',
        list_id: 'work',
        parent_id: 'work-parent',
        scheduled_date: '2026-05-19',
      },
      {
        ...baseTask,
        id: 'hobby-parent',
        list_id: 'hobby',
        scheduled_date: '2026-05-19',
      },
      {
        ...baseTask,
        id: 'hobby-child',
        list_id: 'hobby',
        parent_id: 'hobby-parent',
        scheduled_date: '2026-05-19',
      },
    ];
    const result = filterCarriedOver(tasks, {
      cascadeEnabledFor: (listId) => listId === 'work',
    });
    const ids = result.map((t) => t.id).sort();
    expect(ids).toEqual(['hobby-child', 'hobby-parent', 'work-parent']);
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

// ── Day-start TASK REMINDERS (today = 2026-05-20) ───────────────────────────
describe('filterUntimedToday', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('picks open tasks scheduled today with NO time-of-day', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'untimed', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'timed', scheduled_date: '2026-05-20', scheduled_time: '09:00' },
      { ...baseTask, id: 'yesterday', scheduled_date: '2026-05-19' },
      { ...baseTask, id: 'tomorrow', scheduled_date: '2026-05-21' },
      { ...baseTask, id: 'done', scheduled_date: '2026-05-20', status: 'completed' },
    ];
    expect(filterUntimedToday(tasks).map((t) => t.id)).toEqual(['untimed']);
  });

  it('suppresses a project parent with an open subtask', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'parent', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'child', parent_id: 'parent', status: 'open' },
    ];
    expect(filterUntimedToday(tasks).map((t) => t.id)).toEqual([]);
  });
});

describe('filterDeadlineArrived', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('picks open tasks whose deadline is today (even if already scheduled today)', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'due', deadline_date: '2026-05-20' },
      { ...baseTask, id: 'due-pinned', deadline_date: '2026-05-20', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'yesterday', deadline_date: '2026-05-19' },
      { ...baseTask, id: 'tomorrow', deadline_date: '2026-05-21' },
      { ...baseTask, id: 'done', deadline_date: '2026-05-20', status: 'cancelled' },
    ];
    expect(filterDeadlineArrived(tasks).map((t) => t.id).sort()).toEqual(['due', 'due-pinned']);
    // Unlike the pin selector, the already-pinned one is still surfaced.
    expect(filterDeadlinePinTargets(tasks).map((t) => t.id)).toEqual(['due']);
  });
});

describe('filterDeadlineCountdown', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('is CUMULATIVE — window 3 matches deadlines 3, 2 AND 1 days out (not 4, not today)', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'in4', deadline_date: '2026-05-24' }, // 4 days > window
      { ...baseTask, id: 'in3', deadline_date: '2026-05-23' },
      { ...baseTask, id: 'in2', deadline_date: '2026-05-22' },
      { ...baseTask, id: 'in1', deadline_date: '2026-05-21' },
      { ...baseTask, id: 'today', deadline_date: '2026-05-20' }, // 0 days → filterDeadlineArrived
      { ...baseTask, id: 'past', deadline_date: '2026-05-19' },
      { ...baseTask, id: 'in2-done', deadline_date: '2026-05-22', status: 'completed' },
    ];
    expect(filterDeadlineCountdown(tasks, 3).map((t) => t.id)).toEqual([
      'in3',
      'in2',
      'in1',
    ]);
  });

  it('crosses a month boundary correctly (window 13 covers 06-02 = 13 days out)', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'in13', deadline_date: '2026-06-02' }, // 13 days → in window
      { ...baseTask, id: 'in14', deadline_date: '2026-06-03' }, // 14 days → out
    ];
    expect(filterDeadlineCountdown(tasks, 13).map((t) => t.id)).toEqual(['in13']);
  });

  it('selects nothing for window <= 0 or non-finite', () => {
    const tasks: Task[] = [{ ...baseTask, id: 'in2', deadline_date: '2026-05-22' }];
    expect(filterDeadlineCountdown(tasks, 0)).toEqual([]);
    expect(filterDeadlineCountdown(tasks, -3)).toEqual([]);
    expect(filterDeadlineCountdown(tasks, Number.NaN)).toEqual([]);
  });

  it('a per-task override WIDENS the window (5 days out matches at override 7 but not global 3)', () => {
    const tasks: Task[] = [
      // Override 7 → 5 days out is within 1..7.
      {
        ...baseTask,
        id: 'override7',
        deadline_date: '2026-05-25',
        deadline_reminder_days: 7,
      },
      // No override → global window 3; 5 days out is beyond it.
      { ...baseTask, id: 'global', deadline_date: '2026-05-25' },
    ];
    expect(filterDeadlineCountdown(tasks, 3).map((t) => t.id)).toEqual(['override7']);
  });

  it('falls back to the global window when the override is null or < 1', () => {
    const tasks: Task[] = [
      // null override → global 3; 2 days out is in window.
      { ...baseTask, id: 'null', deadline_date: '2026-05-22', deadline_reminder_days: null },
      // < 1 override → ignored, global 3; 2 days out is in window.
      { ...baseTask, id: 'zero', deadline_date: '2026-05-22', deadline_reminder_days: 0 },
    ];
    expect(filterDeadlineCountdown(tasks, 3).map((t) => t.id).sort()).toEqual([
      'null',
      'zero',
    ]);
  });

  it('honours an override window even when the global default is invalid (<= 0)', () => {
    const tasks: Task[] = [
      // Override 5 → 4 days out is in 1..5, even with the global disabled.
      { ...baseTask, id: 'override5', deadline_date: '2026-05-24', deadline_reminder_days: 5 },
      // No override + invalid global → nothing.
      { ...baseTask, id: 'noov', deadline_date: '2026-05-24' },
    ];
    expect(filterDeadlineCountdown(tasks, 0).map((t) => t.id)).toEqual(['override5']);
  });
});

describe('buildReminderGroups (de-dup, today = 2026-05-20)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('a task due today AND scheduled today (untimed) surfaces only as due-today', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'both', deadline_date: '2026-05-20', scheduled_date: '2026-05-20' },
    ];
    const g = buildReminderGroups(tasks, allRemindersOn);
    expect(g.dueToday.map((t) => t.id)).toEqual(['both']);
    expect(g.untimed).toEqual([]);
    expect(reminderCount(g)).toBe(1); // counted once, not twice
  });

  it('a task planned today (untimed) with a future countdown deadline surfaces only as planned', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'plan', scheduled_date: '2026-05-20', deadline_date: '2026-05-23' },
    ];
    const g = buildReminderGroups(tasks, allRemindersOn);
    expect(g.untimed.map((t) => t.id)).toEqual(['plan']);
    expect(g.countdown).toEqual([]);
    expect(reminderCount(g)).toBe(1);
  });

  it('keeps three distinct tasks in their three groups', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'u', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'd', deadline_date: '2026-05-20' },
      { ...baseTask, id: 'c', deadline_date: '2026-05-23' },
    ];
    const g = buildReminderGroups(tasks, allRemindersOn);
    expect(g.untimed.map((t) => t.id)).toEqual(['u']);
    expect(g.dueToday.map((t) => t.id)).toEqual(['d']);
    expect(g.countdown.map((t) => t.id)).toEqual(['c']);
    expect(reminderCount(g)).toBe(3);
  });

  it('respects the per-group toggles', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'u', scheduled_date: '2026-05-20' },
      { ...baseTask, id: 'd', deadline_date: '2026-05-20' },
    ];
    const g = buildReminderGroups(tasks, { ...allRemindersOn, remindUntimedToday: false });
    expect(g.untimed).toEqual([]);
    expect(g.dueToday.map((t) => t.id)).toEqual(['d']);
  });
});

describe('daysUntilDeadline (today = 2026-05-20)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('counts whole local days to the deadline (0 today, negative past, null none)', () => {
    expect(daysUntilDeadline({ ...baseTask, deadline_date: '2026-05-23' })).toBe(3);
    expect(daysUntilDeadline({ ...baseTask, deadline_date: '2026-05-21' })).toBe(1);
    expect(daysUntilDeadline({ ...baseTask, deadline_date: '2026-05-20' })).toBe(0);
    expect(daysUntilDeadline({ ...baseTask, deadline_date: '2026-05-19' })).toBe(-1);
    expect(daysUntilDeadline({ ...baseTask, deadline_date: '2026-06-02' })).toBe(13);
    expect(daysUntilDeadline({ ...baseTask, deadline_date: null })).toBeNull();
  });
});

describe('day-key anchored groups (the mobile ahead-of-time scheduler)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 20, 12, 0, 0)); // 2026-05-20
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('anchors untimed / due-today to the given future day', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'u', scheduled_date: '2026-05-22' },
      { ...baseTask, id: 'd', deadline_date: '2026-05-22' },
      { ...baseTask, id: 'today', scheduled_date: '2026-05-20' },
    ];
    const groups = buildReminderGroups(tasks, allRemindersOn, undefined, '2026-05-22');
    expect(groups.dueToday.map((t) => t.id)).toEqual(['d']);
    expect(groups.untimed.map((t) => t.id)).toEqual(['u']);
    expect(reminderCount(groups)).toBe(2);
  });

  it('anchors the countdown window to the given day', () => {
    const tasks: Task[] = [{ ...baseTask, id: 'c', deadline_date: '2026-05-25' }];
    // From the 22nd the deadline is 3 days out — inside the window of 3 …
    expect(
      buildReminderGroups(tasks, allRemindersOn, undefined, '2026-05-22').countdown.map(
        (t) => t.id,
      ),
    ).toEqual(['c']);
    // … from today (the 20th) it is 5 days out — outside.
    expect(buildReminderGroups(tasks, allRemindersOn).countdown).toEqual([]);
  });

  it('daysUntilDeadline honours the anchor', () => {
    expect(
      daysUntilDeadline({ ...baseTask, deadline_date: '2026-05-25' }, '2026-05-22'),
    ).toBe(3);
  });

  it('defaults to today when no anchor is given (desktop callers unchanged)', () => {
    const tasks: Task[] = [{ ...baseTask, id: 'now', scheduled_date: '2026-05-20' }];
    expect(
      buildReminderGroups(tasks, allRemindersOn).untimed.map((t) => t.id),
    ).toEqual(['now']);
  });
});
