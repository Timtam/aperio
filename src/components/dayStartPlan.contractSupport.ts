// Test support for the day-start plan contract: what the day-start sites
// compose from the day-start rules every morning — the desktop
// DayStartReviewChecker, the mobile useDayStartChecks and the mobile
// dayStartSchedule — reduced to plain data in and plain data out.
//
// The composition is not a function in the app. `measuredPlan` below is the
// desktop checker's inline block, copied verbatim, with the silent batch's
// target collection from its `runAutoCarryOverBatch`, so the fixture records
// what the app does rather than what a rewrite would. The mobile checks run the
// same block; the mobile scheduler runs it for a future day and keeps the
// slipped rows whose list says exactly 'ask', which is the same set, because
// both surfaces only ever hold one of the three validated values. Nothing in
// the app imports this.
import type { Task, TaskUser } from '../api/types';
import {
  actionableDescendantsOf,
  buildReminderGroups,
  filterCarriedOver,
  filterOverdue,
  reminderCount,
  type ReminderGroups,
  type ReminderSettings,
} from '@aperio/shared';

import { onDay, user } from './dayStart.contractSupport';

export type CarryOverDefault = 'ask' | 'today' | 'backlog';

/** What the composition reads of a list's settings. */
export interface EffectiveListSettings {
  cascade: boolean;
  carryOverDefault: CarryOverDefault;
}

export interface MeasuredPlan {
  overdue: Task[];
  askRows: Task[];
  todayRows: Task[];
  backlogRows: Task[];
  todayTargets: Task[];
  backlogTargets: Task[];
  reminders: ReminderGroups;
  surfaced: number;
}

/**
 * The silent batch's target set, verbatim from `runAutoCarryOverBatch` in
 * `src/components/DayStartReviewChecker.tsx` (the mobile twin in
 * `useDayStartChecks.ts` is the same loop).
 */
function collectTargets(
  slippedRoots: Task[],
  allTasks: Task[],
  effectiveForList: (listId: string) => EffectiveListSettings,
): Task[] {
  // One question for every coupled root: a question per root sent the whole
  // task list across the door each time.
  const coupled = slippedRoots.filter((root) => effectiveForList(root.list_id).cascade);
  const below = actionableDescendantsOf(
    coupled.map((root) => root.id),
    allTasks,
  );
  const belowRoot = new Map<Task, Task[]>(coupled.map((root, i) => [root, below[i]]));
  const collected = new Map<string, Task>();
  for (const root of slippedRoots) {
    collected.set(root.id, root);
    for (const desc of belowRoot.get(root) ?? []) {
      collected.set(desc.id, desc);
    }
  }
  return [...collected.values()];
}

/**
 * The day-start composition, verbatim from the effect in
 * `DayStartReviewChecker`. `dayKey` is left out on the live checkers (today)
 * and given by the scheduler (a future morning).
 */
export function measuredPlan(
  tasks: Task[],
  settings: ReminderSettings,
  effectiveForList: (listId: string) => EffectiveListSettings,
  meFor: (listId: string) => TaskUser | null,
  dayKey?: string,
): MeasuredPlan {
  const overdue = filterOverdue(tasks, meFor, dayKey);
  const slipped = filterCarriedOver(
    tasks,
    {
      cascadeEnabledFor: (listId) => effectiveForList(listId).cascade,
      meFor,
    },
    dayKey,
  );
  const askRows: Task[] = [];
  const todayRows: Task[] = [];
  const backlogRows: Task[] = [];
  for (const row of slipped) {
    const eff = effectiveForList(row.list_id);
    if (eff.carryOverDefault === 'today') todayRows.push(row);
    else if (eff.carryOverDefault === 'backlog') backlogRows.push(row);
    else askRows.push(row);
  }
  const reminderGroups = buildReminderGroups(tasks, settings, meFor, dayKey);
  const reminderTotal = reminderCount(reminderGroups);
  const surfaced = overdue.length + askRows.length + reminderTotal;
  return {
    overdue,
    askRows,
    todayRows,
    backlogRows,
    todayTargets: collectTargets(todayRows, tasks, effectiveForList),
    backlogTargets: collectTargets(backlogRows, tasks, effectiveForList),
    reminders: reminderGroups,
    surfaced,
  };
}

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
  return (listId: string): EffectiveListSettings => {
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
    const plan = measuredPlan(
      tasks,
      i.reminders,
      effectiveOf(i.lists),
      meForOf(i.currentUserByList),
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
