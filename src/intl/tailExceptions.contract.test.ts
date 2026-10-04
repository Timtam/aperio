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
    expect(rows.length).toBeGreaterThanOrEqual(37);
    for (const name of [
      'A1 an evening deletion stays on its day when the series becomes all-day',
      'B2 every second week keeps the deletion it still meets',
      'C a deleted third Monday becomes a deleted third Tuesday',
      'C4 "every Monday" kept by hand on a Tuesday start keeps its deletion on Monday',
      '"every weekday" kept by hand on a moved start keeps its deletions',
      'B a rule set to the moved weekday is the moved rule',
      '"every weekday" moved with the start keeps its places',
      'a rule that follows its start, written out by the repeat field, is the moved rule',
      'without a clock in common a day is the one the device shows',
      'a monthly rule from the 31st moved to the 30th keeps its deletion on its day',
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
