import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/dayStartPlan.json';
import type { Task } from '../api/types';
import { answerPlan, type PlanInput } from './dayStartPlan.contractSupport';

/**
 * The day-start plan, pinned as a table before it moves.
 *
 * What the day-start sites compose from the day-start rules every morning:
 * the overdue tasks, the slipped rows split by each list's carry-over default,
 * the silent batches' targets, the reminder groups, and the count that opens
 * the review. Today that is written inline three times; it is going to be one
 * question to `cal-core`, and the answer has to be the same answer, in the
 * same order. This file replays every case through the composition as the
 * desktop checker writes it — the Rust side will read the same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
describe('dayStartPlan contract', () => {
  const base = contract.baseTask as Task;

  // Anti-silence: named rows, not a count.
  it('still carries the rows the plan turns on', () => {
    const names = new Set(contract.cases.map((c) => c.name));
    for (const needed of [
      'per-list-overrides-split-one-morning',
      'overdue-counts-whatever-the-list-carries',
      'the-count-adds-all-three-sections',
      'a-coupled-root-brings-its-actionable-descendants',
      'a-descendant-that-is-also-a-row-is-targeted-once',
      'a-subtask-in-a-backlog-list-under-a-today-root-is-in-both-batches',
      'the-scheduler-plans-a-future-morning',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const c of contract.cases) {
    it(c.name, () => {
      expect(answerPlan(c.input as PlanInput, base), c.note).toEqual(c.expect);
    });
  }
});
