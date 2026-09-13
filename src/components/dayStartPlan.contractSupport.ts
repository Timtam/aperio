// Test support for the day-start plan contract: what the day-start sites ask
// every morning — the desktop DayStartReviewChecker, the mobile
// useDayStartChecks and the mobile dayStartSchedule — reduced to plain data in
// and plain data out.
//
// The fixture was measured from the composition those sites wrote inline,
// copied verbatim into this file before the port. The answers now come from
// the shell's `planDayStart`, and so from `cal_core::day_start::plan`; rows the
// port changed or added on purpose say so in their notes. Nothing in the app
// imports this.
import type { Task, TaskUser } from '../api/types';
import {
  planDayStart,
  type CarryOverDefault,
  type DayStartListChoice,
  type ReminderSettings,
} from '@aperio/shared';

import { onDay, user } from './dayStart.contractSupport';

export interface ListSettingsInput {
  /** The global coupling (`tasks.cascadeStatusCoupling`). */
  cascade: boolean;
  /** The global carry-over default (`tasks.carryOverDefault`). */
  carryOverDefault: CarryOverDefault;
  /** Per-list overrides; an absent field inherits the global. */
  overrides?: Record<string, { cascade?: boolean; carryOverDefault?: CarryOverDefault }>;
}

export interface PlanInput {
  /** The wall-clock day. */
  today: string;
  /** The morning the scheduler plans ahead for. Absent: the live checkers. */
  anchor?: string;
  /** The account's own user id per list. A list missing from the map has no
   *  identity. Absent: every list has none. */
  currentUserByList?: Record<string, string | null>;
  lists: ListSettingsInput;
  reminders: ReminderSettings;
  /** Task overrides over the fixture's base task. */
  tasks: Partial<Task>[];
}

export interface PlanIds {
  overdue: string[];
  ask: string[];
  today: string[];
  backlog: string[];
  todayTargets: string[];
  backlogTargets: string[];
  untimed: string[];
  dueToday: string[];
  countdown: string[];
  surfaced: number;
}

/** Resolve a list's settings the way both surfaces do: override per field,
 *  else the global. */
export function effectiveOf(lists: ListSettingsInput) {
  return (listId: string): DayStartListChoice => {
    const o = lists.overrides?.[listId];
    return {
      cascade: o?.cascade ?? lists.cascade,
      carryOverDefault: o?.carryOverDefault ?? lists.carryOverDefault,
    };
  };
}

function meForOf(users: Record<string, string | null> | undefined) {
  return (listId: string): TaskUser | null => {
    const id = users?.[listId];
    return id == null ? null : user(id);
  };
}

const ids = (tasks: Task[]): string[] => tasks.map((t) => t.id);

export function answerPlan(i: PlanInput, base: Task): PlanIds {
  return onDay(i.today, () => {
    const tasks = i.tasks.map((over) => ({ ...base, ...over }) as Task);
    const plan = planDayStart(
      tasks,
      {
        reminders: i.reminders,
        listSettings: effectiveOf(i.lists),
        meFor: meForOf(i.currentUserByList),
      },
      i.anchor,
    );
    return {
      overdue: ids(plan.overdue),
      ask: ids(plan.askRows),
      today: ids(plan.todayRows),
      backlog: ids(plan.backlogRows),
      todayTargets: ids(plan.todayTargets),
      backlogTargets: ids(plan.backlogTargets),
      untimed: ids(plan.reminders.untimed),
      dueToday: ids(plan.reminders.dueToday),
      countdown: ids(plan.reminders.countdown),
      surfaced: plan.surfaced,
    };
  });
}
