import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/tailExceptions.json';
import { tailExceptions, type TailExceptionsQuestion } from '@aperio/shared';

/**
 * The deleted occurrences a series keeps from a cut on (decisions 152, 188,
 * 189), row by row through the REAL door.
 *
 * The core decides which deletion goes where; this surface only expands the
 * two series for it (`tailRecurrenceFor`). So each row is asked of
 * `cal_core::tail_exceptions` in WebAssembly — the same answer the phone gets
 * through its own door — and a door that stops reaching the rule, or a field
 * renamed on either side, fails here.
 */

interface Row {
  name: string;
  question: Omit<TailExceptionsQuestion, 'standing'> & { standing?: string[] };
  expected: unknown;
}

const rows = contract.rows as unknown as Row[];

describe('the deleted occurrences a series keeps from a cut on', () => {
  it('covers the cases the decisions turn on', () => {
    expect(rows.length).toBeGreaterThanOrEqual(17);
    for (const name of [
      'A1 an evening deletion stays on its day when the series becomes all-day',
      'B2 every second week keeps the deletion it still meets',
      'C a deleted third Monday becomes a deleted third Tuesday',
      'C4 "every Monday" moved to a Tuesday keeps its deletion on Monday',
    ]) {
      expect(rows.map((row) => row.name)).toContain(name);
    }
  });

  for (const row of rows) {
    it(row.name, () => {
      expect(tailExceptions({ standing: [], ...row.question })).toEqual(row.expected);
    });
  }
});
