import { describe, expect, it } from 'vitest';

import contract from '../../../crates/cal-core/tests/fixtures/taskGrouping.json';
import type { Task } from '../../api/types';
import { isTaskDeferred } from './taskGrouping';
import { answer, type WireInput } from './taskGrouping.contractSupport';

/**
 * The task-view grouping, pinned as a table before it moves into the core.
 *
 * `buildEntries` decides which group a task lands in, in which order, under
 * which header, at what depth, and whether a collapse hides it. Today that
 * decision is TypeScript, asked on both surfaces while rendering; it is going
 * to be asked of `cal-core` instead, and the Rust answer has to be the same
 * answer. This file replays every case in the fixture through the TypeScript
 * and compares — the Rust side reads the same file. While both exist, the
 * fixture is what keeps them together; once the TypeScript goes, it is the
 * record of what the rule did on the day it moved.
 *
 * What the fixture pins is the DECISION, not the wording: a header row carries
 * its kind, its count(s) and the ids it points at, never the title. The titles
 * ("Erledigt (3)", "Inbox (2)") are built by the caller from those — which is
 * exactly the split the core needs, because it cannot hold the `t` callback.
 */
describe('taskGrouping contract', () => {
  const base = contract.baseTask as Task;

  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rule turns on', () => {
    const names = new Set(contract.cases.map((c) => c.name));
    for (const needed of [
      'done-split-when-someone-else-owns-one',
      'time-groups-in-order',
      'sections-by-name-not-by-order',
      'parent-cycle-surfaces-instead-of-vanishing',
      'done-falls-back-to-updated-at',
      'two-level-scale-has-two-bands',
      'collapsing-a-list-hides-its-sections-and-tasks',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const c of contract.cases) {
    it(c.name, () => {
      expect(answer(c.input as WireInput, base), c.note).toEqual(c.expect.rows);
    });
  }

  it('isTaskDeferred rows hold', () => {
    for (const row of contract.deferred) {
      expect(
        isTaskDeferred({ ...base, resurface_date: row.resurface_date } as Task, row.today),
        row.note,
      ).toBe(row.deferred);
    }
  });
});
