import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/taskStatus.json';
import type { Task } from '../api/types';
import { answerKeys, answerProgress, type ProgressInput } from './taskStatus.contractSupport';

/**
 * The two task-status answers that move into the core, pinned as a table
 * before they do: the i18n key for every state, effort and priority (in both
 * scales), and a parent's subtask progress. This file replays the fixture
 * through the TypeScript; the Rust side reads the same file.
 *
 * What stays out — the glyphs, the `t`-wording, the CSS token — is written in
 * the fixture's `notInThisTable`.
 */
describe('taskStatus contract', () => {
  const base = contract.baseTask as Task;

  it('announces every state, effort and priority with the pinned key', () => {
    expect(answerKeys()).toEqual(contract.keys);
  });

  // Anti-silence: the rows the rules turn on, named.
  it('still carries the rows the rules turn on', () => {
    // Two-level: only the top level is announced, as "important".
    expect(contract.keys.priority.two.high).toBe('views.tasks.priorityImportant');
    expect(contract.keys.priority.two.low).toBeNull();
    // Three-level: the middle is silent.
    expect(contract.keys.priority.three.medium).toBeNull();
    expect(contract.keys.effort.medium).toBeNull();
    const names = new Set(contract.progress.map((c) => c.name));
    for (const needed of [
      'counts-completed-and-drops-cancelled-from-the-total',
      'direct-children-only',
      'only-cancelled-children-is-null',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const c of contract.progress) {
    it(`progress: ${c.name}`, () => {
      expect(answerProgress(c.input as ProgressInput, base), c.note).toEqual(c.expect);
    });
  }
});
