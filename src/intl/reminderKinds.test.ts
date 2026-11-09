import { describe, expect, it } from 'vitest';

import {
  afterStartLabelKey,
  allDayKindFor,
  allDayKindOption,
  afterStartOffered,
  allDayDefaultKind,
  allDayDefaultMinutes,
  beforeEndOffered,
  isFromEnd,
  allDayReminderOf,
  atStartLabelKey,
  clockOfText,
  clockText,
  isAfterStart,
  minutesBeforeOfAllDay,
  reminderKindFor,
  reminderKindOption,
  relativeAmount,
  splitRelativeMinutes,
} from '../../shared/reminderKinds';

describe('reminderKindOption — what a stored reminder shows as', () => {
  it('reads zero minutes before as "at start"', () => {
    expect(reminderKindOption({ type: 'relative', minutes_before: 0 })).toBe('at_start');
  });

  it('keeps every other relative reminder before start "before start"', () => {
    expect(reminderKindOption({ type: 'relative', minutes_before: 1 })).toBe('relative');
    expect(reminderKindOption({ type: 'relative', minutes_before: 1440 })).toBe('relative');
  });

  it('reads fewer than zero minutes before as "after start" (222)', () => {
    expect(reminderKindOption({ type: 'relative', minutes_before: -10 })).toBe('after_start');
    expect(reminderKindOption({ type: 'relative', minutes_before: -1 })).toBe('after_start');
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

  it('starts "after start" at 15 minutes after', () => {
    expect(reminderKindFor('after_start', now)).toEqual({ type: 'relative', minutes_before: -15 });
  });

  it('starts a fixed time at the next full hour tomorrow', () => {
    const kind = reminderKindFor('absolute', now);
    expect(kind.type).toBe('absolute');
    const at = new Date(kind.type === 'absolute' ? kind.at : '');
    expect([at.getDate(), at.getHours(), at.getMinutes(), at.getSeconds()]).toEqual([10, 14, 0, 0]);
  });

  it('round-trips: what a kind becomes shows as that kind', () => {
    for (const option of ['relative', 'at_start', 'after_start', 'absolute', 'app_start'] as const) {
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

describe('afterStartOffered — only where it arrives', () => {
  it('is offered where the calendar keeps it, whatever the placement', () => {
    expect(afterStartOffered({ attach: true }, { keptByCalendar: true, placement: true })).toBe(true);
    expect(afterStartOffered({}, { keptByCalendar: true, placement: false })).toBe(true);
  });

  it('is offered on a calendar that drops it only for a row that stays in Aperio', () => {
    const google = { keptByCalendar: false, placement: true };
    expect(afterStartOffered({ attach: false }, google)).toBe(true);
    expect(afterStartOffered({ attach: true }, google)).toBe(false);
    // A default stored before the placement choice existed has no flag: it
    // stays in Aperio, and the editor shows it so.
    expect(afterStartOffered({}, google)).toBe(true);
    // No placement choice (a device calendar): nothing to keep it in.
    expect(afterStartOffered({ attach: false }, { keptByCalendar: false, placement: false })).toBe(false);
  });

  it('names the reminders it rules on', () => {
    expect(isAfterStart({ type: 'relative', minutes_before: -540 })).toBe(true);
    expect(isAfterStart({ type: 'relative', minutes_before: 0 })).toBe(false);
    expect(isAfterStart({ type: 'email', minutes_before: -5 })).toBe(false);
    expect(afterStartLabelKey('event')).toBe('reminders.kind.afterStart');
    expect(afterStartLabelKey('task')).toBe('reminders.kind.afterDue');
  });
});

describe('allDayReminderOf / minutesBeforeOfAllDay — a day and a clock time', () => {
  // The reading of crates/host-core all_day_fire_instant, AllDayReading::Exact.
  const cases: [number, number, number | null, string][] = [
    [-540, 0, 540, 'iPhone: On day of event (9 AM), PT9H'],
    [900, -1, 540, 'iPhone: 1 day before (9 AM), -PT15H'],
    [2340, -2, 540, 'iPhone: 2 days before (9 AM)'],
    [9540, -7, 540, 'iPhone: 1 week before (9 AM)'],
    [420, -1, 1020, 'Google: the day before at 17:00'],
    [-1080, 0, 1080, 'on the day at 18:00'],
    [60, -1, 1380, '1 hour before: 23:00 the evening before'],
    [-1980, 1, 540, 'the day after at 09:00'],
    [0, 0, null, 'on the day, day-start time'],
    [1440, -1, null, '1 day before, day-start time'],
    [-1440, 1, null, '1 day after, day-start time'],
  ];

  it.each(cases)('%i minutes before is day %i at %s (%s)', (minutes, day, clock) => {
    expect(allDayReminderOf(minutes)).toEqual({ day, clock });
  });

  it.each(cases)('and back: day and time give %i again', (minutes) => {
    expect(minutesBeforeOfAllDay(allDayReminderOf(minutes))).toBe(minutes);
  });

  it('takes 00:00 as the whole day, at the day-start time', () => {
    expect(minutesBeforeOfAllDay({ day: -1, clock: 0 })).toBe(1440);
    expect(minutesBeforeOfAllDay({ day: 0, clock: 0 })).toBe(0);
    expect(Object.is(minutesBeforeOfAllDay({ day: 0, clock: null }), 0)).toBe(true);
  });
});

describe('allDayDefaultMinutes — a default on an all-day appointment', () => {
  it('rounds to whole days, half away from zero, as the planner does', () => {
    expect(allDayDefaultMinutes(15)).toBe(0);
    expect(allDayDefaultMinutes(60)).toBe(0);
    expect(allDayDefaultMinutes(720)).toBe(1440);
    expect(allDayDefaultMinutes(-720)).toBe(-1440);
    expect(allDayDefaultMinutes(1440)).toBe(1440);
    expect(allDayDefaultMinutes(900)).toBe(1440);
    expect(Object.is(allDayDefaultMinutes(-60), 0)).toBe(true);
  });
});

describe('clockText / clockOfText', () => {
  it('writes and reads HH:MM', () => {
    expect(clockText(540)).toBe('09:00');
    expect(clockText(1439)).toBe('23:59');
    expect(clockOfText('9:05')).toBe(545);
    expect(clockOfText(' 18:00 ')).toBe(1080);
    expect(clockOfText('9.30')).toBe(570);
  });

  it('refuses what is no time of day', () => {
    expect(clockOfText('24:00')).toBeNull();
    expect(clockOfText('12:60')).toBeNull();
    expect(clockOfText('12')).toBeNull();
    expect(clockOfText('')).toBeNull();
  });
});

describe('allDayKindOption / allDayKindFor — the kinds of an all-day row', () => {
  const rel = (minutes_before: number) => ({ type: 'relative' as const, minutes_before });
  const now = new Date(2026, 10, 9, 14, 0, 0);

  it('reads a reminder by its day', () => {
    expect(allDayKindOption(rel(-540))).toBe('on_day');
    expect(allDayKindOption(rel(0))).toBe('on_day');
    expect(allDayKindOption(rel(900))).toBe('days_before');
    expect(allDayKindOption(rel(60))).toBe('days_before');
    expect(allDayKindOption(rel(-1980))).toBe('days_after');
    expect(allDayKindOption({ type: 'app_start' })).toBe('app_start');
  });

  it('starts each kind at its whole day, and round-trips', () => {
    expect(allDayKindFor('on_day', now)).toEqual(rel(0));
    expect(allDayKindFor('days_before', now)).toEqual(rel(1440));
    expect(allDayKindFor('days_after', now)).toEqual(rel(-1440));
    for (const option of ['on_day', 'days_before', 'days_after', 'absolute', 'app_start'] as const) {
      expect(allDayKindOption(allDayKindFor(option, now))).toBe(option);
    }
  });
});

describe('counting from the end (RFC 5545 RELATED=END, 2026-11-09)', () => {
  const now = new Date('2026-11-09T10:20:00Z');

  it('reads a stored reminder from the end as before, at or after the end', () => {
    expect(reminderKindOption({ type: 'relative_to_end', minutes_before: 15 })).toBe('before_end');
    expect(reminderKindOption({ type: 'relative_to_end', minutes_before: 0 })).toBe('at_end');
    expect(reminderKindOption({ type: 'relative_to_end', minutes_before: -10 })).toBe('after_end');
    expect(isFromEnd({ type: 'relative_to_end', minutes_before: 5 })).toBe(true);
    expect(isFromEnd({ type: 'relative', minutes_before: 5 })).toBe(false);
  });

  it('starts each of them at 15 minutes, or at the end itself', () => {
    expect(reminderKindFor('before_end', now)).toEqual({ type: 'relative_to_end', minutes_before: 15 });
    expect(reminderKindFor('at_end', now)).toEqual({ type: 'relative_to_end', minutes_before: 0 });
    expect(reminderKindFor('after_end', now)).toEqual({ type: 'relative_to_end', minutes_before: -15 });
    for (const option of ['before_end', 'at_end', 'after_end'] as const) {
      expect(reminderKindOption(reminderKindFor(option, now))).toBe(option);
    }
  });

  it('is offered where the calendar keeps it, or for a row that stays in Aperio', () => {
    expect(beforeEndOffered({ attach: true }, { keptByCalendar: true, placement: true })).toBe(true);
    const google = { keptByCalendar: false, placement: true };
    expect(beforeEndOffered({ attach: false }, google)).toBe(true);
    expect(beforeEndOffered({ attach: true }, google)).toBe(false);
    expect(beforeEndOffered({}, { keptByCalendar: false, placement: false })).toBe(false);
  });

  it('shows on an all-day row as before, at or after the end', () => {
    expect(allDayKindOption({ type: 'relative_to_end', minutes_before: 60 })).toBe('before_end');
    expect(allDayKindOption({ type: 'relative_to_end', minutes_before: 0 })).toBe('at_end');
    expect(allDayKindOption({ type: 'relative_to_end', minutes_before: -60 })).toBe('after_end');
  });

  it('rounds a default from the end up to whole days, at least one, on an all-day row', () => {
    const day = 24 * 60;
    const end = (minutes_before: number) => ({ type: 'relative_to_end' as const, minutes_before });
    expect(allDayDefaultKind(end(15))).toEqual(end(day));
    expect(allDayDefaultKind(end(0))).toEqual(end(day));
    expect(allDayDefaultKind(end(day))).toEqual(end(day));
    expect(allDayDefaultKind(end(day + 1))).toEqual(end(2 * day));
    expect(allDayDefaultKind(end(-day))).toEqual(end(-day));
  });
});
