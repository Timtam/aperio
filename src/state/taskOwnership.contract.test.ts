import { describe, expect, it } from 'vitest';

import contract from '../../shared/contracts/taskOwnership.json';
import { isMineOrUnassigned } from '@aperio/shared';
import type { TaskUser } from '@aperio/shared';

/**
 * The TypeScript half of the task-ownership contract.
 *
 * Its Rust twin lives in `crates/host-core/src/reminders.rs`
 * (`ownership_contract`) and reads the SAME file. Unlike the calendar-default
 * -reminders contract beside it, this one pins a DECISION rather than a wire
 * format: each side answers "is this task mine to act on?" independently, on
 * the same data, and neither can see the other's answer.
 *
 * That independence is what makes a disagreement expensive. Rust answers it
 * before a reminder Trigger is ever built, so a wrong `true` rings the phone
 * for a task this app does not list as mine, and a wrong `false` silences one
 * it does. Nothing crashes and nothing is logged — it would take days of
 * noticing that the two surfaces disagree.
 */

interface OwnershipCase {
  name: string;
  note?: string;
  assignees: string[];
  me: string | null;
  mine: boolean;
}

const user = (id: string): TaskUser => ({
  id,
  name: `name of ${id}`,
  email: null,
});

describe('task ownership — the contract Rust reads too', () => {
  const cases = contract.cases as OwnershipCase[];

  // Name, don't count: a floor would pass on exactly the edit that empties
  // this file, and each case below names itself when it fails.
  it('the contract says something', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  it.each(cases)('$name', ({ assignees, me, mine }) => {
    expect(isMineOrUnassigned(assignees.map(user), me === null ? null : user(me))).toBe(
      mine,
    );
  });

  it('still contains the colleague\'s task', () => {
    // The only `false` case, and the whole reason the rule exists. Without it
    // the contract would pass against a rule that answers `true` for
    // everything — which is the bug it is here to catch.
    expect(cases.some((c) => c.mine === false)).toBe(true);
  });

  it('decides on the user id alone, not on the name or the mail', () => {
    // Providers hand back the same person with different display names per
    // endpoint. Matching on anything but the id would drop a real assignment —
    // and this is the property the fixture cannot express, because it carries
    // ids only.
    const me: TaskUser = { id: 'u1', name: 'Toni', email: 'a@example.org' };
    const samePersonOtherLabel: TaskUser = { id: 'u1', name: 'T. B.', email: null };
    expect(isMineOrUnassigned([samePersonOtherLabel], me)).toBe(true);
  });
});
