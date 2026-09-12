import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/taskDay.json';
import type { Task } from '../api/types';
import {
  answerOnDay,
  answerSplit,
  answerWeeks,
  type OnDayInput,
  type SplitInput,
  type WeeksInput,
} from './taskDay.contractSupport';

/**
 * The calendar-day task rules, pinned as a table before they move.
 *
 * `filterTasksOnDay` decides which tasks a day shows and in which order;
 * `taskTimeOnDay`, `taskEndTimeOnDay` and `isDeadlineChip` say what the chip
 * carries; `backlogWeeks` and `splitDeadlinesByWeek` cut the backlog rail.
 * Today that is TypeScript, asked on both surfaces while rendering; it is going
 * to be asked of `cal-core` instead, and the Rust answer has to be the same
 * answer. This file replays every case in the fixture through the TypeScript
 * and compares — the Rust side reads the same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
describe('taskDay contract', () => {
  const base = contract.baseTask as Task;

  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rules turn on', () => {
    const onDay = new Set(contract.onDay.map((c) => c.name));
    for (const needed of [
      'scheduled-and-due-shows-only-on-the-plan-day',
      'completed-without-a-plan-day-sits-on-its-completion-day',
      'completed-keeps-the-day-it-was-planned-for',
      'owner-filter-hides-a-colleagues-task',
      'time-scheduled-wins-over-deadline-on-the-same-day',
      'two-level-scale-has-two-bands',
    ]) {
      expect(onDay, `fixture lost ${needed}`).toContain(needed);
    }
    const weeks = new Set(contract.weeks.map((c) => c.name));
    expect(weeks).toContain('crosses-month-and-year');
    const split = new Set(contract.deadlineSplit.map((c) => c.name));
    expect(split).toContain('overdue-stays-with-this-week');
  });

  for (const c of contract.onDay) {
    it(`on a day: ${c.name}`, () => {
      expect(answerOnDay(c.input as OnDayInput, base), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.weeks) {
    it(`weeks: ${c.name}`, () => {
      expect(answerWeeks(c.input as WeeksInput), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.deadlineSplit) {
    it(`split: ${c.name}`, () => {
      expect(answerSplit(c.input as SplitInput), c.note).toEqual(c.expect);
    });
  }
});
