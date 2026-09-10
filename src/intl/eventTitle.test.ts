import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/normalizedTitle.json';
import { normalizedTitle } from '@aperio/shared';

/**
 * The title rule, against the SAME table `cal_core::normalized_title` answers.
 *
 * Unlike conference detection, this rule still has two implementations. That is
 * not an oversight: the callers that need it here — `findGroupSuggestions` and
 * `suggestGroupMate` — are themselves on their way into `cal-core`, and they
 * take the TypeScript half with them when they go. A door built for the one
 * step in between would be thrown away on the next.
 *
 * The third caller has already gone that way: `healEventGroups` was deleted
 * when the host took over anchoring group membership, which is what this
 * arrangement is for.
 *
 * So until then this file carries the whole weight. Two independent
 * implementations of one decision is exactly the arrangement that let five
 * copies drift; the fixture is what makes the drift loud instead of silent.
 */
interface ContractCase {
  title: string;
  normalized: string;
  note: string;
}

describe('the title rule answers the core contract', () => {
  const cases = contract.cases as ContractCase[];

  it('reads the same fixture the Rust contract test reads', () => {
    // Anti-silence: an import resolving to an empty object would make every
    // case below vacuous and leave the suite green while proving nothing.
    // Named rather than counted — a floor would be today's row count, and
    // adding a row is the change this assert should survive.
    expect(cases.map((c) => c.title)).toContain('Wochen  planung');
  });

  it.each(cases)('$note', ({ title, normalized }) => {
    expect(normalizedTitle(title)).toBe(normalized);
  });

  it('is idempotent', () => {
    // Both anchor-repair sites compare a STORED signature against a live title.
    // A rule that moved on a second pass would make that comparison depend on
    // how many times each side had been through it.
    for (const { title } of cases) {
      const once = normalizedTitle(title);
      expect(normalizedTitle(once)).toBe(once);
    }
  });

  it('gives the same answer whichever gap a provider re-flowed the title with', () => {
    // The reason the rule exists, stated as a property rather than as rows: the
    // gaps in a title are not part of what it says, so every way of writing the
    // gap has to reach one answer. A regression here does not throw — it just
    // stops offering a group, or drops a member out of one, in silence.
    const gaps = [' ', '  ', '\t', '\n', '\r\n', '\u0085', '\u00a0', ' \n\t '];
    const answers = new Set(gaps.map((gap) => normalizedTitle(`Wochen${gap}planung`)));
    expect([...answers]).toEqual(['wochen planung']);
  });
});
