import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/taskOccurrences.json';
import type { Task } from '../api/types';
import {
  answerExpand,
  answerMove,
  answerNext,
  type ExpandInput,
  type MoveInput,
  type NextInput,
} from './expandTaskOccurrences.contractSupport';

/**
 * The recurring-task projection, pinned as a table before it moves.
 *
 * `expandScheduledRecurringTasks` replaces a recurring scheduled task with its
 * occurrences inside a window — the real task on its own day, read-only
 * projections on every other — and passes everything else through;
 * `nextTaskOccurrence` is one step of the same walk; `occurrenceMoveTarget`
 * says what "move to this day" can be on a source that owns the date. Today
 * that is TypeScript, run on both surfaces while rendering the calendar; it is
 * going to be asked of `cal-core` instead, and the Rust answer has to be the
 * same answer, in the same order. This file replays every case through the
 * TypeScript — the Rust side reads the same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
describe('taskOccurrences contract', () => {
  const base = contract.baseTask as Task;

  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rules turn on', () => {
    const expand = new Set(contract.expand.map((c) => c.name));
    for (const needed of [
      'daily-projects-every-day-in-the-window',
      'the-real-task-is-the-one-on-its-own-day',
      'weekly-by-day-projects-only-the-listed-weekdays',
      'interval-with-named-weekdays-is-not-dropped',
      'monthly-day-of-month-clamps-to-short-months',
      'far-past-daily-base-reaches-the-window',
      'stops-at-the-until-bound',
      'completed-passes-through',
      'from-completion-passes-through',
      'a-fixed-date-with-day-32-is-dropped',
      'a-base-beyond-the-step-cap-emits-nothing',
      'an-empty-scheduled-date-is-undated',
    ]) {
      expect(expand, `fixture lost ${needed}`).toContain(needed);
    }
    const next = new Set(contract.next.map((c) => c.name));
    expect(next).toContain('past-an-until-end-is-null');
    expect(next).toContain('all-invalid-fixed-dates-fall-back-to-the-frequency');
    const move = new Set(contract.move.map((c) => c.name));
    expect(move).toContain('advances-the-series-where-the-source-owns-the-date');
    expect(move).toContain('an-ended-series-keeps-the-requested-day');
  });

  for (const c of contract.expand) {
    it(`expand: ${c.name}`, () => {
      expect(answerExpand(c.input as ExpandInput, base), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.next) {
    it(`next: ${c.name}`, () => {
      expect(answerNext(c.input as NextInput), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.move) {
    it(`move: ${c.name}`, () => {
      expect(answerMove(c.input as MoveInput, base), c.note).toEqual(c.expect);
    });
  }
});
