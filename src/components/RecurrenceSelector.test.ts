import { describe, expect, it } from 'vitest';
import {
  buildRRule,
  defaultUntilDay,
  deriveMonthlyOptions,
  parseRRule,
  pickerMisreadsRule,
  resolveAgainstDay,
  withEndMode,
} from './rrule';

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
  untilText: '',
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

describe('the end of a rule (UNTIL)', () => {
  // DESIGN-series-time-zone.md, "Prüfen und Speichern": an untouched end
  // stays as it was written. The controls rebuild the whole rule on every
  // change, and rewrote any end as the end of its UTC day: a cut's bound
  // one second before an occurrence then reached past it again.
  it('goes back as it came when another control changes', () => {
    for (const raw of ['20261102T075959Z', '20261031', '20261031T090000', '20261231t235959z']) {
      const parsed = parseRRule(`FREQ=WEEKLY;BYDAY=MO;UNTIL=${raw}`);
      expect(parsed.untilText).toBe(raw);
      const rebuilt = buildRRule({ ...parsed, interval: 2, byDay: ['MO', 'WE'] });
      expect(rebuilt).toBe(`FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=${raw}`);
    }
  });

  it('is written as the end of its UTC day once its date changes', () => {
    const parsed = parseRRule('FREQ=DAILY;UNTIL=20261102T075959Z');
    expect(parsed.until).toBe('2026-11-02');
    expect(buildRRule({ ...parsed, until: '2026-11-20' })).toBe(
      'FREQ=DAILY;UNTIL=20261120T235959Z',
    );
  });

  it('is never written from half a date', () => {
    for (const until of ['2026-11-3', '2026-1-5', '2026-11']) {
      const rebuilt = buildRRule({ ...base, freq: 'DAILY', endMode: 'UNTIL', until });
      expect(rebuilt).toBe('FREQ=DAILY');
      const fromStored = buildRRule({
        ...parseRRule('FREQ=DAILY;UNTIL=20261130T235959Z'),
        until,
      });
      expect(fromStored).toBe('FREQ=DAILY');
    }
  });

  it('keeps an end it cannot read, while nobody changes it', () => {
    const parsed = parseRRule('FREQ=DAILY;UNTIL=soon');
    expect(parsed).toMatchObject({ endMode: 'UNTIL', until: '', untilText: 'soon' });
    expect(buildRRule({ ...parsed, interval: 3 })).toBe('FREQ=DAILY;INTERVAL=3;UNTIL=soon');
  });

  it('leaves the controls reading the rule as it is stored', () => {
    for (const rule of [
      'FREQ=DAILY;UNTIL=20261102T075959Z',
      'FREQ=WEEKLY;BYDAY=MO;UNTIL=20261031',
      'FREQ=WEEKLY;BYDAY=MO;UNTIL=20261031T090000',
    ]) {
      expect(pickerMisreadsRule(rule)).toBe(false);
    }
  });
});

describe('choosing "until a date"', () => {
  it('starts on the day of the tenth occurrence, where the count would end', () => {
    const weekly = { ...base, freq: 'WEEKLY' as const, byDay: ['MO'] };
    expect(defaultUntilDay(weekly, '2026-11-02')).toBe('2027-01-04');
    expect(defaultUntilDay({ ...base, freq: 'DAILY' }, '2026-11-02')).toBe('2026-11-11');
    const monthly = { ...base, freq: 'MONTHLY' as const, byMonthDay: 31 };
    // Months without a 31st are skipped, as the rule skips them.
    expect(defaultUntilDay(monthly, '2026-10-31')).toBe('2028-01-31');
  });

  it('falls back to the start day where the day cannot be read', () => {
    expect(defaultUntilDay({ ...base, freq: 'DAILY' }, 'someday')).toBe('someday');
    expect(defaultUntilDay(base, '2026-11-02')).toBe('2026-11-02');
  });

  it('stays chosen: the rule it writes reads back as ending on that date', () => {
    // Without a date no end was written, and the control read the rule
    // back as "never" at once.
    const rule = withEndMode(parseRRule('FREQ=DAILY'), 'UNTIL', '2026-11-02');
    const written = buildRRule(rule);
    expect(written).toBe('FREQ=DAILY;UNTIL=20261111T235959Z');
    expect(parseRRule(written)).toMatchObject({ endMode: 'UNTIL', until: '2026-11-11' });
  });

  it('keeps a date the rule already has, and leaves other ends alone', () => {
    const stored = parseRRule('FREQ=DAILY;UNTIL=20261102T075959Z');
    expect(withEndMode(stored, 'UNTIL', '2026-11-02')).toEqual(stored);
    expect(withEndMode(parseRRule('FREQ=DAILY'), 'COUNT', '2026-11-02')).toMatchObject({
      endMode: 'COUNT',
      until: '',
    });
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
