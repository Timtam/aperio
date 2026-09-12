import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/taskCascade.json';
import type { Task } from '../api/types';
import {
  answerAutoDate,
  answerCascade,
  answerDerive,
  answerRecompute,
  type AutoDateInput,
  type CascadeInput,
  type DeriveInput,
  type RecomputeInput,
} from './taskCascade.contractSupport';

/**
 * The parent/subtask status coupling, pinned as a table before it moves.
 *
 * `deriveStatusFromChildren` says what a parent is, given its children;
 * `planStatusCascade` plans the writes of a status change (root, descendants,
 * ancestors — in order); `planAncestorRecompute` re-derives after a subtask
 * was created or deleted; `autoDateOnStart` is the "started → today"
 * companion. Today that is TypeScript, run on both surfaces when a task is
 * checked off; it is going to be asked of `cal-core` instead, and the Rust
 * answer has to be the same answer, in the same order. This file replays every
 * case through the TypeScript — the Rust side reads the same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
describe('taskCascade contract', () => {
  const base = contract.baseTask as Task;

  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rules turn on', () => {
    const cascade = new Set(contract.cascade.map((c) => c.name));
    for (const needed of [
      'completing-a-parent-follows-every-non-cancelled-descendant',
      'descendants-come-depth-first-last-child-first',
      'the-climb-stops-where-nothing-changes',
      'an-empty-parent-id-is-no-parent-on-the-way-up',
      'cycle-down-two-tasks',
      'autodate-also-pins-a-dateless-parent-derived-to-in-progress',
      'decoupled-no-up-cascade',
    ]) {
      expect(cascade, `fixture lost ${needed}`).toContain(needed);
    }
    const recompute = new Set(contract.recompute.map((c) => c.name));
    expect(recompute).toContain('the-recompute-climbs-past-an-unchanged-level');
    expect(recompute).toContain('autodate-pins-a-dateless-parent-newly-in-progress');
    const derive = new Map(contract.derive.map((c) => [c.name, c.expect]));
    // The two rows the whole derivation turns on.
    expect(derive.get('presence-completed+cancelled')).toBe('completed');
    expect(derive.get('presence-open+cancelled')).toBe('open');
    expect(derive.get('presence-open+completed')).toBe('in_progress');
    const autoDate = new Set(contract.autoDate.map((c) => c.name));
    expect(autoDate).toContain('a-dateless-task-entering-in-progress-gets-today');
  });

  for (const c of contract.autoDate) {
    it(`auto-date: ${c.name}`, () => {
      expect(answerAutoDate(c.input as AutoDateInput), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.derive) {
    it(`derive: ${c.name}`, () => {
      expect(answerDerive(c.input as DeriveInput, base), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.cascade) {
    it(`cascade: ${c.name}`, () => {
      expect(answerCascade(c.input as CascadeInput, base), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.recompute) {
    it(`recompute: ${c.name}`, () => {
      expect(answerRecompute(c.input as RecomputeInput, base), c.note).toEqual(c.expect);
    });
  }
});
