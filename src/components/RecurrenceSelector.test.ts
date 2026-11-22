import { describe, expect, it } from 'vitest';
import { buildRRule, deriveMonthlyOptions, parseRRule, resolveAgainstDay } from './rrule';

// Shared base so each test only spells out the fields it cares about.
const base = {
  freq: 'NONE' as const,
  interval: 1,
  byDay: [] as string[],
  wkst: '',
  monthlyMode: 'DAY_OF_MONTH' as const,
  byMonthDay: 0,
  relOrdinal: 0,
  relWeekday: '',
  byMonth: 0,
  endMode: 'NEVER' as const,
  count: 10,
  until: '',
};

describe('parseRRule', () => {
  it('returns a none-rule for null', () => {
    expect(parseRRule(null).freq).toBe('NONE');
  });

  it('reads FREQ, INTERVAL and BYDAY', () => {
    const r = parseRRule('FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE');
    expect(r.freq).toBe('WEEKLY');
    expect(r.interval).toBe(2);
    expect(r.byDay).toEqual(['MO', 'WE']);
  });

  it('reads COUNT as the COUNT end mode', () => {
    const r = parseRRule('FREQ=DAILY;COUNT=5');
    expect(r.endMode).toBe('COUNT');
    expect(r.count).toBe(5);
  });

  it('reads UNTIL as YYYY-MM-DD', () => {
    const r = parseRRule('FREQ=DAILY;UNTIL=20260530T235959Z');
    expect(r.endMode).toBe('UNTIL');
    expect(r.until).toBe('2026-05-30');
  });

  it('accepts an RRULE: prefix', () => {
    expect(parseRRule('RRULE:FREQ=DAILY').freq).toBe('DAILY');
  });

  it('reads absolute monthly (BYMONTHDAY)', () => {
    const r = parseRRule('FREQ=MONTHLY;BYMONTHDAY=15');
    expect(r.monthlyMode).toBe('DAY_OF_MONTH');
    expect(r.byMonthDay).toBe(15);
  });

  it('reads relative monthly (BYDAY=3WE) as a weekday rule', () => {
    const r = parseRRule('FREQ=MONTHLY;BYDAY=3WE');
    expect(r.monthlyMode).toBe('WEEKDAY');
    expect(r.relOrdinal).toBe(3);
    expect(r.relWeekday).toBe('WE');
    // The ordinal-prefixed token must NOT leak into the weekly byDay.
    expect(r.byDay).toEqual([]);
  });

  it('reads relative monthly with a negative ordinal (-1FR = last)', () => {
    const r = parseRRule('FREQ=MONTHLY;BYDAY=-1FR');
    expect(r.monthlyMode).toBe('WEEKDAY');
    expect(r.relOrdinal).toBe(-1);
    expect(r.relWeekday).toBe('FR');
  });

  it('reads relative yearly (BYMONTH + BYDAY=1FR)', () => {
    const r = parseRRule('FREQ=YEARLY;BYMONTH=3;BYDAY=1FR');
    expect(r.byMonth).toBe(3);
    expect(r.monthlyMode).toBe('WEEKDAY');
    expect(r.relOrdinal).toBe(1);
    expect(r.relWeekday).toBe('FR');
  });
});

describe('WKST', () => {
  // DESIGN-series-time-zone.md: WKST stays. The controls had no field for
  // it, so touching them dropped it, and a biweekly series whose week
  // starts on Sunday moved to other weeks.
  it('is read and written back as the rule had it', () => {
    const rule = 'FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,TU;WKST=SU';
    const parsed = parseRRule(rule);
    expect(parsed.wkst).toBe('SU');
    expect(buildRRule({ ...parsed, interval: 3 })).toBe(
      'FREQ=WEEKLY;INTERVAL=3;BYDAY=SU,TU;WKST=SU',
    );
  });

  it('is not written where the rule named none', () => {
    expect(parseRRule('FREQ=WEEKLY;BYDAY=MO').wkst).toBe('');
    expect(buildRRule(parseRRule('FREQ=WEEKLY;BYDAY=MO'))).toBe('FREQ=WEEKLY;BYDAY=MO');
  });
});

describe('the day-derived parts (cal_core::day_parts)', () => {
  it('come from the day key, not from a Date on the device clock', () => {
    // 2026-11-24 is the fourth Tuesday of November and in its last week.
    expect(deriveMonthlyOptions('2026-11-24').map((o) => o.key)).toEqual([
      'dom',
      'nth',
      'last',
    ]);
    expect(resolveAgainstDay(parseRRule('FREQ=YEARLY'), '2026-11-24')).toMatchObject({
      byMonthDay: 24,
      relOrdinal: 4,
      relWeekday: 'TU',
      byMonth: 11,
    });
  });

  it('give nothing for a day the core refuses', () => {
    expect(deriveMonthlyOptions('')).toEqual([]);
    expect(deriveMonthlyOptions('2026-02-30')).toEqual([]);
    const rule = parseRRule('FREQ=MONTHLY');
    expect(resolveAgainstDay(rule, 'today')).toBe(rule);
  });
});

describe('buildRRule', () => {
  it('returns null for the none-rule', () => {
    expect(buildRRule({ ...base, freq: 'NONE' })).toBeNull();
  });

  it('omits INTERVAL when it is 1', () => {
    expect(buildRRule({ ...base, freq: 'DAILY' })).toBe('FREQ=DAILY');
  });

  it('writes BYDAY for weekly rules', () => {
    expect(buildRRule({ ...base, freq: 'WEEKLY', byDay: ['MO', 'WE'] })).toBe(
      'FREQ=WEEKLY;BYDAY=MO,WE',
    );
  });

  it('emits an end-of-day UTC UNTIL', () => {
    expect(
      buildRRule({
        ...base,
        freq: 'DAILY',
        endMode: 'UNTIL',
        until: '2026-05-30',
      }),
    ).toBe('FREQ=DAILY;UNTIL=20260530T235959Z');
  });

  it('writes absolute monthly as BYMONTHDAY', () => {
    expect(
      buildRRule({
        ...base,
        freq: 'MONTHLY',
        monthlyMode: 'DAY_OF_MONTH',
        byMonthDay: 15,
      }),
    ).toBe('FREQ=MONTHLY;BYMONTHDAY=15');
  });

  it('writes relative monthly as an ordinal BYDAY', () => {
    expect(
      buildRRule({
        ...base,
        freq: 'MONTHLY',
        monthlyMode: 'WEEKDAY',
        relOrdinal: 3,
        relWeekday: 'WE',
      }),
    ).toBe('FREQ=MONTHLY;BYDAY=3WE');
  });

  it('writes "last weekday" relative monthly with -1', () => {
    expect(
      buildRRule({
        ...base,
        freq: 'MONTHLY',
        monthlyMode: 'WEEKDAY',
        relOrdinal: -1,
        relWeekday: 'FR',
      }),
    ).toBe('FREQ=MONTHLY;BYDAY=-1FR');
  });

  it('writes relative yearly with BYMONTH + ordinal BYDAY', () => {
    expect(
      buildRRule({
        ...base,
        freq: 'YEARLY',
        byMonth: 3,
        monthlyMode: 'WEEKDAY',
        relOrdinal: 1,
        relWeekday: 'FR',
      }),
    ).toBe('FREQ=YEARLY;BYMONTH=3;BYDAY=1FR');
  });

  it('round-trips a weekly rule through parse', () => {
    const original = 'FREQ=WEEKLY;INTERVAL=3;BYDAY=MO,WE,FR;COUNT=10';
    expect(buildRRule(parseRRule(original))).toBe(original);
  });

  it('round-trips a relative monthly rule through parse', () => {
    const original = 'FREQ=MONTHLY;BYDAY=3WE';
    expect(buildRRule(parseRRule(original))).toBe(original);
  });

  it('round-trips a relative yearly rule through parse', () => {
    const original = 'FREQ=YEARLY;BYMONTH=3;BYDAY=1FR';
    expect(buildRRule(parseRRule(original))).toBe(original);
  });
});

describe('deriveMonthlyOptions', () => {
  it('offers day-of-month + nth-weekday for a mid-month date', () => {
    // 2024-05-15 is the third Wednesday of May (not in the last week).
    const opts = deriveMonthlyOptions('2024-05-15');
    expect(opts.map((o) => o.key)).toEqual(['dom', 'nth']);
    expect(opts[0]).toMatchObject({ mode: 'DAY_OF_MONTH', day: 15 });
    expect(opts[1]).toMatchObject({
      mode: 'WEEKDAY',
      ordinal: 3,
      weekday: 'WE',
    });
  });

  it('adds a "last weekday" option when the date is in the final week', () => {
    // 2026-05-29 is a Friday in the last 7 days of May.
    const opts = deriveMonthlyOptions('2026-05-29');
    expect(opts.map((o) => o.key)).toContain('last');
    const last = opts.find((o) => o.key === 'last');
    expect(last).toMatchObject({ mode: 'WEEKDAY', ordinal: -1, weekday: 'FR' });
  });

  it('marks relative options disabled when the source cannot store them', () => {
    // A source that can't store relative recurrence (BYDAY=Nxx):
    // the day-of-month option stays enabled, the weekday options
    // are greyed (still visible) rather than removed.
    const opts = deriveMonthlyOptions('2026-05-29', { relative: false });
    const dom = opts.find((o) => o.key === 'dom');
    const nth = opts.find((o) => o.key === 'nth');
    const last = opts.find((o) => o.key === 'last');
    expect(dom?.disabled).toBeFalsy();
    expect(nth?.disabled).toBe(true);
    expect(last?.disabled).toBe(true);
  });

  it('marks the day-of-month option disabled when the source cannot store one', () => {
    // The other axis, and it had no answer at all: a source that repeats on
    // the item's own day and cannot take a separate day number (Vikunja says
    // so in its manifest) still had "on the 29th" offered on the desktop, and
    // the value was accepted and dropped on save.
    const opts = deriveMonthlyOptions('2026-05-29', {
      dayOfMonth: false,
    });
    expect(opts.find((o) => o.key === 'dom')?.disabled).toBe(true);
    // The relative options are a different axis and stay untouched.
    expect(opts.find((o) => o.key === 'nth')?.disabled).toBeFalsy();
  });

  it('can disable both axes at once', () => {
    const opts = deriveMonthlyOptions('2026-05-29', {
      relative: false,
      dayOfMonth: false,
    });
    expect(opts.every((o) => o.disabled)).toBe(true);
  });

  it('leaves every option enabled by default (full support)', () => {
    // The permissive default the manifest itself uses: an adapter spells out
    // only what it restricts, so an absent block must mean "everything".
    expect(
      deriveMonthlyOptions('2024-05-15').every((o) => !o.disabled),
    ).toBe(true);
    expect(
      deriveMonthlyOptions('2024-05-15', {}).every((o) => !o.disabled),
    ).toBe(true);
  });
});
