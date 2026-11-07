import { describe, expect, it } from 'vitest';

import {
  atStartLabelKey,
  reminderKindFor,
  reminderKindOption,
  relativeAmount,
  splitRelativeMinutes,
} from '../../shared/reminderKinds';

describe('reminderKindOption — what a stored reminder shows as', () => {
  it('reads zero minutes before as "at start"', () => {
    expect(reminderKindOption({ type: 'relative', minutes_before: 0 })).toBe('at_start');
  });

  it('keeps every other relative reminder "before start"', () => {
    expect(reminderKindOption({ type: 'relative', minutes_before: 1 })).toBe('relative');
    expect(reminderKindOption({ type: 'relative', minutes_before: 1440 })).toBe('relative');
    // After the start (222) is not "at start": it stays what it was until it
    // gets a kind of its own.
    expect(reminderKindOption({ type: 'relative', minutes_before: -10 })).toBe('relative');
  });

  it('shows the other kinds as themselves, and e-mail as relative', () => {
    expect(reminderKindOption({ type: 'absolute', at: '2026-11-09T09:00:00Z' })).toBe('absolute');
    expect(reminderKindOption({ type: 'app_start' })).toBe('app_start');
    expect(reminderKindOption({ type: 'email', minutes_before: 0 })).toBe('relative');
  });
});

describe('reminderKindFor — what a chosen kind becomes', () => {
  const now = new Date(2026, 10, 9, 14, 37, 12);

  it('stores "at start" as zero minutes before', () => {
    expect(reminderKindFor('at_start', now)).toEqual({ type: 'relative', minutes_before: 0 });
  });

  it('goes back to 15 minutes before', () => {
    expect(reminderKindFor('relative', now)).toEqual({ type: 'relative', minutes_before: 15 });
  });

  it('starts a fixed time at the next full hour tomorrow', () => {
    const kind = reminderKindFor('absolute', now);
    expect(kind.type).toBe('absolute');
    const at = new Date(kind.type === 'absolute' ? kind.at : '');
    expect([at.getDate(), at.getHours(), at.getMinutes(), at.getSeconds()]).toEqual([10, 14, 0, 0]);
  });

  it('round-trips: what a kind becomes shows as that kind', () => {
    for (const option of ['relative', 'at_start', 'absolute', 'app_start'] as const) {
      expect(reminderKindOption(reminderKindFor(option, now))).toBe(option);
    }
  });
});

describe('atStartLabelKey — the words where the row stands', () => {
  it('says "when due" for a task, whatever the day', () => {
    expect(atStartLabelKey('task', false)).toBe('reminders.kind.atDue');
    expect(atStartLabelKey('task', true)).toBe('reminders.kind.atDue');
  });

  it('says "at start" for an appointment and "on the day" for an all-day one', () => {
    expect(atStartLabelKey('event', false)).toBe('reminders.kind.atStart');
    expect(atStartLabelKey('event', true)).toBe('reminders.kind.onTheDay');
  });
});

describe('relativeAmount — what a typed amount counts as', () => {
  it('cuts to a whole number before checking, so it is never zero', () => {
    // A pasted 0.5 and "15" turned into 0.15 by a comma before the first digit.
    expect(relativeAmount(0.5)).toBe(1);
    expect(relativeAmount(0.15)).toBe(1);
    expect(relativeAmount(15.7)).toBe(15);
  });

  it('takes 1 for nothing, zero, below zero and what is no number', () => {
    expect(relativeAmount(0)).toBe(1);
    expect(relativeAmount(-3)).toBe(1);
    expect(relativeAmount(Number.NaN)).toBe(1);
    expect(relativeAmount(Number.POSITIVE_INFINITY)).toBe(1);
  });
});

describe('splitRelativeMinutes', () => {
  it('picks the largest whole unit', () => {
    expect(splitRelativeMinutes(2880)).toEqual({ amount: 2, unit: 'days' });
    expect(splitRelativeMinutes(120)).toEqual({ amount: 2, unit: 'hours' });
    expect(splitRelativeMinutes(90)).toEqual({ amount: 90, unit: 'minutes' });
  });
});
