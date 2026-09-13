// Test support for the day-start contract: the selectors of shared/dayStart.ts
// that move into the core, reduced to plain data in and plain data out. Shared
// by the contract test and the one-off measure test that wrote the fixture;
// nothing in the app imports this.
//
// Every answer is computed with the local wall clock at noon of the case's
// `today`, because three of the selectors read the clock themselves
// (`movedToToday`, `filterDeadlinePinTargets`, and every selector whose day
// parameter is left out). The core will take that day as a parameter.
import { vi } from 'vitest';

import type { Task, TaskUser } from '../api/types';
import {
  actionableDescendants,
  buildReminderGroups,
  daysUntilDeadline,
  filterCarriedOver,
  filterDeadlineArrived,
  filterDeadlineCountdown,
  filterDeadlinePinTargets,
  filterOverdue,
  filterUntimedToday,
  hasActionableDescendants,
  movedToToday,
  shouldFireToday,
  type ReminderSettings,
} from '@aperio/shared';

/** A user is its id here; the ownership rule compares ids alone. */
export function user(id: string): TaskUser {
  return { id, name: `Name of ${id}`, email: null };
}

/** Run `fn` with the local wall clock at noon of `today` (YYYY-MM-DD). */
export function onDay<T>(today: string, fn: () => T): T {
  const [y, m, d] = today.split('-').map(Number);
  vi.useFakeTimers();
  vi.setSystemTime(new Date(y, m - 1, d, 12, 0, 0));
  try {
    return fn();
  } finally {
    vi.useRealTimers();
  }
}

export interface Scope {
  /** The wall-clock day. */
  today: string;
  /** A day passed to the selector explicitly — the mobile scheduler anchors
   *  FUTURE days this way. Absent: the selector's own default, today. */
  anchor?: string;
  /** The account's own user id per list. Absent: no ownership filter at all.
   *  A list missing from the map answers "no identity". */
  currentUserByList?: Record<string, string | null>;
}

export type SelectInput = Scope & {
  /** Task overrides over the fixture's base task. */
  tasks: Partial<Task>[];
};

function hydrate(rows: Partial<Task>[], base: Task): Task[] {
  return rows.map((over) => ({ ...base, ...over }) as Task);
}

function meForOf(scope: Scope): ((listId: string) => TaskUser | null) | undefined {
  const users = scope.currentUserByList;
  if (users === undefined) return undefined;
  return (listId: string) => {
    const id = users[listId];
    return id == null ? null : user(id);
  };
}

const ids = (tasks: Task[]): string[] => tasks.map((t) => t.id);

export function answerOverdue(i: SelectInput, base: Task): string[] {
  return onDay(i.today, () => {
    const tasks = hydrate(i.tasks, base);
    return ids(
      i.anchor === undefined
        ? filterOverdue(tasks, meForOf(i))
        : filterOverdue(tasks, meForOf(i), i.anchor),
    );
  });
}

export type CarriedOverInput = SelectInput & {
  /** Lists whose status coupling (cascade) is on. Absent: the option is not
   *  passed at all. */
  cascadeLists?: string[];
};

export function answerCarriedOver(i: CarriedOverInput, base: Task): string[] {
  return onDay(i.today, () => {
    const tasks = hydrate(i.tasks, base);
    const meFor = meForOf(i);
    const cascade = i.cascadeLists;
    const options = {
      ...(cascade === undefined
        ? {}
        : { cascadeEnabledFor: (listId: string) => cascade.includes(listId) }),
      ...(meFor === undefined ? {} : { meFor }),
    };
    return ids(
      i.anchor === undefined
        ? filterCarriedOver(tasks, options)
        : filterCarriedOver(tasks, options, i.anchor),
    );
  });
}

export interface TreeInput {
  tasks: Partial<Task>[];
  rootId: string;
}

export function answerActionableDescendants(i: TreeInput, base: Task): string[] {
  return ids(actionableDescendants(i.rootId, hydrate(i.tasks, base)));
}

export function answerHasActionableDescendants(i: TreeInput, base: Task): boolean {
  return hasActionableDescendants(i.rootId, hydrate(i.tasks, base));
}

export interface MoveInput {
  today: string;
  task: Partial<Task>;
}

export interface MovedDates {
  deadline_date: string | null;
  deadline_time: string | null;
  scheduled_date: string | null;
  scheduled_time: string | null;
}

export function answerMovedToToday(i: MoveInput, base: Task): MovedDates {
  return onDay(i.today, () => {
    const moved = movedToToday({ ...base, ...i.task } as Task);
    return {
      deadline_date: moved.deadline_date,
      deadline_time: moved.deadline_time,
      scheduled_date: moved.scheduled_date,
      scheduled_time: moved.scheduled_time,
    };
  });
}

export function answerDeadlinePinTargets(i: SelectInput, base: Task): string[] {
  return onDay(i.today, () => ids(filterDeadlinePinTargets(hydrate(i.tasks, base), meForOf(i))));
}

export interface DaysInput {
  today: string;
  anchor?: string;
  deadline: string | null;
}

export function answerDaysUntilDeadline(i: DaysInput, base: Task): number | null {
  return onDay(i.today, () => {
    const task = { ...base, deadline_date: i.deadline } as Task;
    return i.anchor === undefined ? daysUntilDeadline(task) : daysUntilDeadline(task, i.anchor);
  });
}

export function answerUntimedToday(i: SelectInput, base: Task): string[] {
  return onDay(i.today, () => {
    const tasks = hydrate(i.tasks, base);
    return ids(
      i.anchor === undefined
        ? filterUntimedToday(tasks, meForOf(i))
        : filterUntimedToday(tasks, meForOf(i), i.anchor),
    );
  });
}

export function answerDeadlineArrived(i: SelectInput, base: Task): string[] {
  return onDay(i.today, () => {
    const tasks = hydrate(i.tasks, base);
    return ids(
      i.anchor === undefined
        ? filterDeadlineArrived(tasks, meForOf(i))
        : filterDeadlineArrived(tasks, meForOf(i), i.anchor),
    );
  });
}

export type CountdownInput = SelectInput & {
  /** The global countdown window (`tasks.deadlineCountdownDays`). */
  daysUntil: number;
};

export function answerDeadlineCountdown(i: CountdownInput, base: Task): string[] {
  return onDay(i.today, () => {
    const tasks = hydrate(i.tasks, base);
    return ids(
      i.anchor === undefined
        ? filterDeadlineCountdown(tasks, i.daysUntil, meForOf(i))
        : filterDeadlineCountdown(tasks, i.daysUntil, meForOf(i), i.anchor),
    );
  });
}

export type GroupsInput = SelectInput & { settings: ReminderSettings };

export interface GroupIds {
  untimed: string[];
  dueToday: string[];
  countdown: string[];
}

export function answerReminderGroups(i: GroupsInput, base: Task): GroupIds {
  return onDay(i.today, () => {
    const tasks = hydrate(i.tasks, base);
    const g =
      i.anchor === undefined
        ? buildReminderGroups(tasks, i.settings, meForOf(i))
        : buildReminderGroups(tasks, i.settings, meForOf(i), i.anchor);
    return { untimed: ids(g.untimed), dueToday: ids(g.dueToday), countdown: ids(g.countdown) };
  });
}

export interface FireInput {
  trigger: string;
  lastFired: string | null;
  today: string;
  /** The local wall-clock time, `HH:MM`. */
  now: string;
}

export function answerShouldFire(i: FireInput): boolean {
  const [y, m, d] = i.today.split('-').map(Number);
  const [hh, mm] = i.now.split(':').map(Number);
  return shouldFireToday(i.trigger, i.lastFired, i.today, new Date(y, m - 1, d, hh, mm, 0));
}
