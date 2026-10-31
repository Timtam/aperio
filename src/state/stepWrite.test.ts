import { describe, expect, it } from 'vitest';

import { createRunsInSteps, createsInSteps, writeWithoutWaiting } from '@aperio/shared';

/**
 * Where a series is created in steps (decisions 227, 265): the target
 * calendar's adapter deletes a new series' deleted occurrences one by one, and
 * the series carries some.
 */
describe('createsInSteps', () => {
  const exchange = { recurrence_capabilities: { deletions_in_steps: true } };
  const caldav = { recurrence_capabilities: { deletions_in_steps: false } };
  const withDeletions = { rrule: 'FREQ=WEEKLY', exceptions: ['2026-11-09T09:00:00Z'] };

  it('holds for a series with deletions in a calendar that writes them in steps', () => {
    expect(createsInSteps(exchange, withDeletions)).toBe(true);
  });

  it('never for a calendar that takes them with the series, an unknown one, or none', () => {
    expect(createsInSteps(caldav, withDeletions)).toBe(false);
    expect(createsInSteps({}, withDeletions)).toBe(false);
    expect(createsInSteps(undefined, withDeletions)).toBe(false);
    expect(createsInSteps({ recurrence_capabilities: null }, withDeletions)).toBe(false);
  });

  it('never for a series without deletions, or an appointment without a rule', () => {
    expect(createsInSteps(exchange, { rrule: 'FREQ=WEEKLY', exceptions: [] })).toBe(false);
    expect(createsInSteps(exchange, { rrule: 'FREQ=WEEKLY' })).toBe(false);
    expect(createsInSteps(exchange, { rrule: null, exceptions: ['x'] })).toBe(false);
    expect(createsInSteps(exchange, null)).toBe(false);
  });

  it('never for a create that keeps someone else’s meeting as theirs', () => {
    // Review round 21 of #128: Exchange and Google take it with its
    // deletions; nothing is removed one occurrence at a time.
    const own = 'me@example.org';
    expect(createRunsInSteps(exchange, { recurrence: withDeletions }, own)).toBe(true);
    expect(
      createRunsInSteps(exchange, { recurrence: withDeletions, keeps_invitation: false }, own),
    ).toBe(true);
    expect(
      createRunsInSteps(exchange, { recurrence: withDeletions, keeps_invitation: true }, own),
    ).toBe(false);
    expect(createRunsInSteps(caldav, { recurrence: withDeletions }, own)).toBe(false);
  });

  it('waits for an invitation where the account’s address was not known', () => {
    // Review round 22 of #128: read as someone else's for want of the
    // address, it may be the account's own, which the adapter writes in steps.
    const invitation = { recurrence: withDeletions, keeps_invitation: true };
    expect(createRunsInSteps(exchange, invitation, null)).toBe(true);
    expect(createRunsInSteps(exchange, invitation, undefined)).toBe(true);
    expect(createRunsInSteps(exchange, invitation, ' ')).toBe(true);
  });

  it('a surface without a dialog just writes', async () => {
    await expect(writeWithoutWaiting(true, async () => 7)).resolves.toBe(7);
    await expect(
      writeWithoutWaiting(false, async () => {
        throw new Error('no');
      }),
    ).rejects.toThrow('no');
  });
});
