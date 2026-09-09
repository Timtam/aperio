import { describe, expect, it } from 'vitest';

import {
  isImportantPriority,
  normalPriority,
  priorityRank,
  taskOrder,
  type PriorityScale,
  type Task,
  type TaskPriority,
} from '@aperio/shared';

/**
 * The priority rules, exercised through the door the desktop actually uses.
 *
 * This replaces `coreRules.parity.test.ts`, which asked "does the Rust answer
 * what the TypeScript answers?" — the right question while both existed. There
 * is one implementation now (`cal_core::task_priority`), so that test would
 * have compared Rust against Rust and passed for the wrong reason. Deleting it
 * rather than letting it stand was the point of the move.
 *
 * What replaces it is the same shape the collation got: `src/test-setup.ts`
 * installs the real WebAssembly module as the surface's `TaskPriorityRules`, so
 * every assertion here runs the core. The expected values are SPELLED OUT
 * rather than computed, so a change to the Rust rule shows up here as a failing
 * number and not as two sides agreeing on something new.
 *
 * The whole input space is enumerated — three priorities times two scales is
 * six — which is possible precisely because these rules carry no policy: no
 * i18n keys, no glyphs. Those stayed in the frontend.
 *
 * Proven red by sabotage: swapping the two-level ranking in
 * `crates/cal-core/src/task_priority.rs` turns the two-level rows below into
 * `expected 1 to be 0`.
 */
describe('the priority ranking comes from the core', () => {
  const ALL: TaskPriority[] = ['high', 'medium', 'low'];

  it('ranks all three levels in the three-level scale', () => {
    const ranks = Object.fromEntries(
      ALL.map((p) => [p, priorityRank(p, 'three')]),
    );
    expect(ranks).toEqual({ high: 0, medium: 1, low: 2 });
  });

  it('puts low and medium in ONE band in the two-level scale', () => {
    // Not a rounding of the three-level answer. The two are indistinguishable
    // on screen there, so ranking them apart would order a list by an attribute
    // the reader cannot perceive — and the A-to-Z tiebreak each band promises
    // would appear to break at random.
    const ranks = Object.fromEntries(ALL.map((p) => [p, priorityRank(p, 'two')]));
    expect(ranks).toEqual({ high: 0, medium: 1, low: 1 });
  });

  it('defaults to the three-level scale', () => {
    // The callers are comparators handed straight to `Array.sort`; one that
    // omits the scale must sort exactly as the app always did.
    for (const p of ALL) {
      expect(priorityRank(p)).toBe(priorityRank(p, 'three'));
    }
  });

  it('calls only the top priority important', () => {
    expect(ALL.map(isImportantPriority)).toEqual([true, false, false]);
  });

  it('keeps what the task already had when important is cleared', () => {
    // The whole point of the rule: unchecking must not silently promote `low`
    // to `medium`. The two look identical in the two-level system and
    // different in the three-level one the user may switch back to.
    expect(normalPriority('low')).toBe('low');
    expect(normalPriority('medium')).toBe('medium');
    expect(normalPriority('high')).toBe('medium');
    expect(normalPriority(null)).toBe('medium');
    expect(normalPriority(undefined)).toBe('medium');
  });

  it('orders a task list by band, then A to Z inside it', () => {
    const task = (id: string, title: string, priority: TaskPriority): Task =>
      ({ id, list_id: 'L1', title, priority, status: 'open' }) as Task;
    const order = (scale: PriorityScale) =>
      [
        task('c', 'Zebra', 'low'),
        task('a', 'Apfel', 'medium'),
        task('b', 'Brot', 'high'),
      ]
        .sort((x, y) => taskOrder(x, y, scale))
        .map((t) => t.title);

    // Three bands: high, then medium, then low.
    expect(order('three')).toEqual(['Brot', 'Apfel', 'Zebra']);
    // Two bands: the important one, then everything else A to Z — which is
    // what collapsing low and medium buys.
    expect(order('two')).toEqual(['Brot', 'Apfel', 'Zebra']);
  });
});
