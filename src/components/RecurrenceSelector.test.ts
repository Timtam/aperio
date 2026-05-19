import { describe, expect, it } from 'vitest';
import { buildRRule, parseRRule } from './RecurrenceSelector';

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
});

describe('buildRRule', () => {
  it('returns null for the none-rule', () => {
    expect(
      buildRRule({
        freq: 'NONE',
        interval: 1,
        byDay: [],
        endMode: 'NEVER',
        count: 10,
        until: '',
      }),
    ).toBeNull();
  });

  it('omits INTERVAL when it is 1', () => {
    expect(
      buildRRule({
        freq: 'DAILY',
        interval: 1,
        byDay: [],
        endMode: 'NEVER',
        count: 10,
        until: '',
      }),
    ).toBe('FREQ=DAILY');
  });

  it('writes BYDAY for weekly rules', () => {
    expect(
      buildRRule({
        freq: 'WEEKLY',
        interval: 1,
        byDay: ['MO', 'WE'],
        endMode: 'NEVER',
        count: 10,
        until: '',
      }),
    ).toBe('FREQ=WEEKLY;BYDAY=MO,WE');
  });

  it('emits an end-of-day UTC UNTIL', () => {
    expect(
      buildRRule({
        freq: 'DAILY',
        interval: 1,
        byDay: [],
        endMode: 'UNTIL',
        count: 10,
        until: '2026-05-30',
      }),
    ).toBe('FREQ=DAILY;UNTIL=20260530T235959Z');
  });

  it('round-trips through parse', () => {
    const original = 'FREQ=WEEKLY;INTERVAL=3;BYDAY=MO,WE,FR;COUNT=10';
    expect(buildRRule(parseRRule(original))).toBe(original);
  });
});
