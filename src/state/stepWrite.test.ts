import { describe, expect, it } from 'vitest';

import { createsInSteps, writeWithoutWaiting } from '@aperio/shared';

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

  it('a surface without a dialog just writes', async () => {
    await expect(writeWithoutWaiting(true, async () => 7)).resolves.toBe(7);
    await expect(
      writeWithoutWaiting(false, async () => {
        throw new Error('no');
      }),
    ).rejects.toThrow('no');
  });
});
