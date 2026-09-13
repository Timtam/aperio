import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/taskAssignment.json';
import {
  answerClamp,
  answerMode,
  answerSelfAssign,
  type ClampInput,
  type ModeInput,
  type SelfAssignInput,
} from './taskAssignment.contractSupport';

/**
 * The assignment rules, pinned as a table before they move.
 *
 * `selfAssignOnStatusChange` says who holds a task after a status change;
 * `taskAssignmentMode` says how many people a list can hold on one task;
 * `clampAssignees` trims a list to that. Today that is TypeScript, run on
 * both surfaces when a task is checked off or moved; it is going to be asked
 * of `cal-core` instead, and the Rust answer has to be the same answer. This
 * file replays every case through the TypeScript — the Rust side reads the
 * same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
describe('taskAssignment contract', () => {
  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rules turn on', () => {
    const selfAssign = new Set(contract.selfAssign.map((c) => c.name));
    for (const needed of [
      'an-unassigned-task-going-in-progress-becomes-mine',
      'a-task-a-colleague-holds-is-not-taken',
      'reopening-removes-only-me',
      'reopening-keeps-the-order-when-i-am-in-the-middle',
      'cancelling-changes-nothing',
      'no-identity-changes-nothing',
    ]) {
      expect(selfAssign, `fixture lost ${needed}`).toContain(needed);
    }
    const mode = new Set(contract.mode.map((c) => c.name));
    expect(mode).toContain('a-capabilities-block-without-the-field-cannot-assign');
    const clamp = new Set(contract.clamp.map((c) => c.name));
    expect(clamp).toContain('single-keeps-the-first');
    expect(clamp).toContain('none-never-empties-what-it-cannot-show');
  });

  for (const c of contract.selfAssign) {
    it(`self-assign: ${c.name}`, () => {
      expect(answerSelfAssign(c.input as SelfAssignInput), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.mode) {
    it(`mode: ${c.name}`, () => {
      expect(answerMode(c.input as ModeInput), c.note).toEqual(c.expect);
    });
  }

  for (const c of contract.clamp) {
    it(`clamp: ${c.name}`, () => {
      expect(answerClamp(c.input as ClampInput), c.note).toEqual(c.expect);
    });
  }
});
