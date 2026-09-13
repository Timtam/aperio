import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/dayStart.json';
import type { Task } from '../api/types';
import { actionableDescendants, actionableDescendantsOf } from '@aperio/shared';
import {
  answerActionableDescendants,
  answerCarriedOver,
  answerDaysUntilDeadline,
  answerDeadlineArrived,
  answerDeadlineCountdown,
  answerDeadlinePinTargets,
  answerHasActionableDescendants,
  answerMovedToToday,
  answerOverdue,
  answerReminderGroups,
  answerShouldFire,
  answerUntimedToday,
  type CarriedOverInput,
  type CountdownInput,
  type DaysInput,
  type FireInput,
  type GroupsInput,
  type MoveInput,
  type SelectInput,
  type TreeInput,
} from './dayStart.contractSupport';

/**
 * The day-start rules, pinned as a table measured from the TypeScript before
 * they moved into `cal_core::day_start`.
 *
 * Overdue, slipped, pinned-to-today, the three reminder groups, the days to a
 * deadline, "move to today" and the fire gate, run on both surfaces every
 * morning. This file replays every case through the shell in
 * `shared/dayStart.ts` and so through the door into the core — the core's own
 * contract test reads the same file. Rows the port changed on purpose say so
 * in their notes.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`; what
 * could not be measured (parent cycles, which hung the TypeScript), in
 * `notMeasurable`.
 */
describe('dayStart contract', () => {
  const base = contract.baseTask as Task;

  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rules turn on', () => {
    const named = (section: { name: string }[], ...needed: string[]) => {
      const names = new Set(section.map((c) => c.name));
      for (const n of needed) expect(names, `fixture lost ${n}`).toContain(n);
    };
    named(contract.overdue, 'a-project-parent-waits-for-its-subtasks', 'ownership-keeps-mine-and-unassigned');
    named(
      contract.carriedOver,
      'overdue-takes-priority',
      'a-project-parent-with-a-lapsed-deadline-and-plan-is-carried-over',
      'cascade-hides-a-grandchild-under-a-slipped-grandparent',
      'cascade-is-decided-by-the-subtasks-own-list',
    );
    named(contract.actionableDescendants, 'the-last-child-subtree-comes-first-below-the-root');
    named(contract.movedToToday, 'lifts-a-lapsed-plan-keeping-its-time');
    named(contract.deadlinePinTargets, 'a-deadline-today-not-yet-planned-today');
    named(contract.daysUntilDeadline, 'a-month-thirteen-is-none', 'over-the-spring-clock-change');
    named(contract.deadlineCountdown, 'the-window-is-cumulative', 'an-override-holds-with-an-invalid-global');
    named(
      contract.reminderGroups,
      'due-today-and-planned-today-counts-once-as-due',
      'with-the-due-toggle-off-a-due-and-planned-task-counts-as-planned',
    );
    named(contract.shouldFireToday, 'garbage-fires-immediately', 'app-start-never-fires-again');
  });

  for (const c of contract.overdue) {
    it(`overdue: ${c.name}`, () => {
      expect(answerOverdue(c.input as SelectInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.carriedOver) {
    it(`carried over: ${c.name}`, () => {
      expect(answerCarriedOver(c.input as CarriedOverInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.actionableDescendants) {
    it(`actionable descendants: ${c.name}`, () => {
      expect(answerActionableDescendants(c.input as TreeInput, base), c.note).toEqual(c.expect);
    });
  }
  it('asks the walk for many roots once, with the answers of one root each', () => {
    for (const c of contract.actionableDescendants) {
      const tasks = c.input.tasks.map((over) => ({ ...base, ...over }) as Task);
      const roots = [...tasks.map((t) => t.id), 'nobody'];
      const batch = actionableDescendantsOf(roots, tasks);
      const singles = roots.map((root) => actionableDescendants(root, tasks));
      expect(batch, c.name).toEqual(singles);
      // The caller's own objects come back, not copies.
      batch.flat().forEach((task) => expect(tasks).toContain(task));
    }
    expect(actionableDescendantsOf([], [])).toEqual([]);
  });

  for (const c of contract.hasActionableDescendants) {
    it(`has actionable descendants: ${c.name}`, () => {
      expect(answerHasActionableDescendants(c.input as TreeInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.movedToToday) {
    it(`moved to today: ${c.name}`, () => {
      expect(answerMovedToToday(c.input as MoveInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.deadlinePinTargets) {
    it(`deadline pin: ${c.name}`, () => {
      expect(answerDeadlinePinTargets(c.input as SelectInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.daysUntilDeadline) {
    it(`days until deadline: ${c.name}`, () => {
      expect(answerDaysUntilDeadline(c.input as DaysInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.untimedToday) {
    it(`untimed today: ${c.name}`, () => {
      expect(answerUntimedToday(c.input as SelectInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.deadlineArrived) {
    it(`deadline arrived: ${c.name}`, () => {
      expect(answerDeadlineArrived(c.input as SelectInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.deadlineCountdown) {
    it(`countdown: ${c.name}`, () => {
      expect(answerDeadlineCountdown(c.input as CountdownInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.reminderGroups) {
    it(`reminder groups: ${c.name}`, () => {
      expect(answerReminderGroups(c.input as GroupsInput, base), c.note).toEqual(c.expect);
    });
  }
  for (const c of contract.shouldFireToday) {
    it(`should fire: ${c.name}`, () => {
      expect(answerShouldFire(c.input as FireInput), c.note).toEqual(c.expect);
    });
  }
});
