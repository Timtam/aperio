import { describe, expect, it } from 'vitest';

import contract from '../../shared/contracts/taskOwnership.json';
import { filterOverdue } from '@aperio/shared';
import type { Task, TaskUser } from '@aperio/shared';

/**
 * The TypeScript half of the task-ownership contract.
 *
 * Its Rust twin lives in `crates/host-core/src/reminders.rs`
 * (`ownership_contract`) and reads the SAME file. Unlike the calendar-default
 * -reminders contract beside it, this one pins a DECISION rather than a wire
 * format: "is this task mine to act on?", asked by the reminder scheduler in
 * the host and by the day start on both surfaces.
 *
 * The rule is `cal_core::is_mine_or_unassigned` on both sides now; the
 * TypeScript copy went when the day start moved into the core. What this side
 * still owns is the shell in front of the day-start door — the account's
 * identity per list, a user reduced to its id — so every case is asked
 * through that door: one overdue task, offered or not. A disagreement stays
 * expensive: a wrong `true` offers a colleague's task in my morning review, a
 * wrong `false` hides one of mine, and nothing crashes or logs.
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

/** Whether the day start offers a lapsed task held by `assignees` to `me`. */
function offered(assignees: TaskUser[], me: TaskUser | null): boolean {
  const task = {
    id: 'task',
    list_id: 'list',
    status: 'open',
    parent_id: null,
    scheduled_date: null,
    scheduled_time: null,
    deadline_date: '2026-05-10',
    deadline_reminder_days: null,
    assignees,
  } as unknown as Task;
  return filterOverdue([task], () => me, '2026-05-20').length === 1;
}

describe('task ownership — the contract Rust reads too', () => {
  const cases = contract.cases as OwnershipCase[];

  // Name, don't count: a floor would pass on exactly the edit that empties
  // this file, and each case below names itself when it fails.
  it('the contract says something', () => {
    expect(cases.length).toBeGreaterThan(0);
  });

  it.each(cases)('$name', ({ assignees, me, mine }) => {
    expect(offered(assignees.map(user), me === null ? null : user(me))).toBe(mine);
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
    expect(offered([samePersonOtherLabel], me)).toBe(true);
  });
});
