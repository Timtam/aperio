import { describe, expect, it } from 'vitest';
import {
  expandEvent,
  expandAll,
  isExpandedOccurrence,
  localTimeZone,
  truncateRRuleBefore,
  splitRRuleForEdit,
  withCreatedRecurrenceZone,
} from './recurrence';
import type { CalendarEvent } from '../api/types';

function mkEvent(overrides: Partial<CalendarEvent> = {}): CalendarEvent {
  const start = '2026-05-19T09:00:00.000Z';
  const end = '2026-05-19T10:00:00.000Z';
  return {
    id: 'evt-1',
    calendar_id: 'cal-1',
    title: 'Standup',
    description: null,
    location: null,
    start,
    end,
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
    created_at: start,
    updated_at: start,
    etag: null,
    ...overrides,
  };
}

const ONE_DAY = 24 * 60 * 60 * 1000;

describe('expandEvent', () => {
  it('returns the event unchanged when there is no recurrence', () => {
    const ev = mkEvent();
    const out = expandEvent(ev, {
      start: new Date('2026-05-01'),
      end: new Date('2026-06-01'),
    });
    expect(out).toEqual([ev]);
  });

  it('expands a weekly rule into the right number of occurrences', () => {
    const ev = mkEvent({
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=TU',
        exceptions: [],
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-01'),
      end: new Date('2026-06-01'),
    });
    // dtstart is Tuesday 2026-05-19. Tuesdays in [dtstart, range end]
    // that fall in range: 19 and 26 May (2 June is past the range end).
    expect(out.length).toBe(2);
    out.forEach((occ) => expect(new Date(occ.start).getUTCDay()).toBe(2));
  });

  it('honours COUNT', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.length).toBe(3);
  });

  it('honours UNTIL', () => {
    const ev = mkEvent({
      recurrence: {
        rrule: 'FREQ=DAILY;UNTIL=20260521T235959Z',
        exceptions: [],
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    // 19, 20, 21 — three occurrences.
    expect(out.length).toBe(3);
  });

  it('skips EXDATE entries', () => {
    const ev = mkEvent({
      recurrence: {
        rrule: 'FREQ=DAILY;COUNT=5',
        exceptions: ['2026-05-20T09:00:00.000Z'],
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.length).toBe(4);
    expect(out.find((o) => o.start.startsWith('2026-05-20'))).toBeUndefined();
  });

  it('synthesises unique IDs and a series_id', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'FREQ=DAILY;COUNT=2', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out[0].id).not.toBe(out[1].id);
    expect(isExpandedOccurrence(out[0])).toBe(true);
    if (isExpandedOccurrence(out[0])) {
      expect(out[0].series_id).toBe('evt-1');
    }
  });

  it('preserves event duration on every occurrence', () => {
    const ev = mkEvent({
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T10:30:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=3', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    out.forEach((occ) => {
      const dur = new Date(occ.end).getTime() - new Date(occ.start).getTime();
      expect(dur).toBe(90 * 60 * 1000);
    });
  });

  it('falls back to the master event if the rule is invalid', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'BOGUS=NOPE', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-01'),
      end: new Date('2026-06-01'),
    });
    expect(out).toEqual([ev]);
  });

  it('accepts a rule with an explicit RRULE: prefix', () => {
    const ev = mkEvent({
      recurrence: { rrule: 'RRULE:FREQ=DAILY;COUNT=2', exceptions: [] },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.length).toBe(2);
  });
});

describe('expandAll', () => {
  it('mixes single and recurring events, sorted chronologically', () => {
    const single = mkEvent({
      id: 'a',
      start: '2026-05-22T08:00:00.000Z',
      end: '2026-05-22T09:00:00.000Z',
    });
    const recurring = mkEvent({
      id: 'b',
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T10:00:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=4', exceptions: [] },
    });
    const out = expandAll([single, recurring], {
      start: new Date('2026-05-19'),
      end: new Date(Date.parse('2026-05-19') + 10 * ONE_DAY),
    });
    expect(out.length).toBe(5); // 4 daily + 1 single
    const starts = out.map((e) => e.start);
    const sorted = [...starts].sort();
    expect(starts).toEqual(sorted);
  });

  it('applies a RECURRENCE-ID override in place of the master occurrence', () => {
    const master = mkEvent({
      id: 'series',
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T09:30:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=4', exceptions: [] },
    });
    // The 2026-05-20 occurrence was moved to 14:00. It arrives as a separate
    // non-recurring event whose id carries the series id + the replaced
    // occurrence (the shape the CalDAV AND Google adapters mint — a bare series
    // id with no `|` is exactly what Google's `{master}::rid::{original}` uses).
    const override = mkEvent({
      id: 'series::rid::2026-05-20T09:00:00Z',
      start: '2026-05-20T14:00:00.000Z',
      end: '2026-05-20T14:30:00.000Z',
      title: 'Standup (moved)',
    });
    const out = expandAll([master, override], {
      start: new Date('2026-05-19'),
      end: new Date(Date.parse('2026-05-19') + 10 * ONE_DAY),
    });
    // 4 daily occurrences, but 05-20 09:00 is replaced by the override:
    // 3 master occurrences + 1 override = 4.
    expect(out.length).toBe(4);
    // The master's own 05-20 09:00 copy is gone…
    expect(
      out.find((o) => o.start === '2026-05-20T09:00:00.000Z'),
    ).toBeUndefined();
    // …and the moved instance stands in for it.
    const moved = out.find((o) => o.start === '2026-05-20T14:00:00.000Z');
    expect(moved?.title).toBe('Standup (moved)');
  });

  it('suppresses the master occurrence for a CANCELLED override too', () => {
    const master = mkEvent({
      id: 'series',
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T09:30:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=4', exceptions: [] },
    });
    // A DELETED single occurrence: Google surfaces it as a cancelled override at
    // the original slot (its id carries the RECURRENCE-ID). expandAll must still
    // drop the master's occurrence; the cancelled override is then hidden by the
    // show-cancelled filter downstream, so the deleted occurrence vanishes.
    const cancelledOverride = mkEvent({
      id: 'series::rid::2026-05-20T09:00:00Z',
      start: '2026-05-20T09:00:00.000Z',
      end: '2026-05-20T09:00:00.000Z',
      cancelled: true,
    });
    const out = expandAll([master, cancelledOverride], {
      start: new Date('2026-05-19'),
      end: new Date(Date.parse('2026-05-19') + 10 * ONE_DAY),
    });
    // The master's own 05-20 09:00 live copy is gone…
    expect(
      out.find((o) => o.start === '2026-05-20T09:00:00.000Z' && !o.cancelled),
    ).toBeUndefined();
    // …leaving 3 live occurrences (the cancelled override is filtered elsewhere).
    expect(out.filter((o) => !o.cancelled).length).toBe(3);
  });

  it('drops the cancelled override entirely so a deleted occurrence VANISHES', () => {
    // Regression: a user-deleted Google occurrence comes back as a cancelled
    // RECURRENCE-ID tombstone (empty, no title). With show-cancelled-events ON
    // (the default) it must NOT render — the occurrence should disappear, not show
    // as an empty cancelled row. expandAll drops it after using it to suppress.
    const master = mkEvent({
      id: 'series',
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T09:30:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=4', exceptions: [] },
    });
    const cancelledOverride = mkEvent({
      id: 'series::rid::2026-05-20T09:00:00Z',
      start: '2026-05-20T09:00:00.000Z',
      end: '2026-05-20T09:00:00.000Z',
      cancelled: true,
    });
    const out = expandAll([master, cancelledOverride], {
      start: new Date('2026-05-19'),
      end: new Date(Date.parse('2026-05-19') + 10 * ONE_DAY),
    });
    // Nothing at all remains on 05-20 — no live copy AND no cancelled tombstone.
    expect(out.some((o) => o.start.startsWith('2026-05-20'))).toBe(false);
    expect(out.some((o) => o.id.includes('::rid::'))).toBe(false);
    // The three other daily occurrences are untouched.
    expect(out.length).toBe(3);
  });

  it('suppresses a cancelled override on a DST-summer occurrence of a zoned master', () => {
    // Reproduces Lea's Google series: a weekly Monday 12:30 Europe/Berlin master
    // whose DTSTART is in WINTER (CET, +1 → 11:30Z) and a single SUMMER occurrence
    // (CEST, +2 → 10:30Z) deleted. Google surfaces the deletion as a cancelled
    // override whose RECURRENCE-ID is the occurrence's DST-correct instant
    // (10:30Z). Suppression only works if the master expands that Monday to the
    // same 10:30Z — i.e. DST-correctly — not to the DTSTART's flat 11:30Z.
    const master = mkEvent({
      id: 'series',
      start: '2026-01-05T11:30:00.000Z', // Mon 2026-01-05 12:30 Berlin (CET)
      end: '2026-01-05T12:00:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=MO',
        exceptions: [],
        tzid: 'Europe/Berlin',
      },
    });
    const cancelledOverride = mkEvent({
      id: 'series::rid::2026-07-13T10:30:00Z',
      start: '2026-07-13T10:30:00.000Z',
      end: '2026-07-13T10:30:00.000Z',
      cancelled: true,
    });
    const out = expandAll([master, cancelledOverride], {
      start: new Date('2026-07-06T00:00:00Z'),
      end: new Date('2026-07-20T00:00:00Z'),
    });
    // The deleted Monday (2026-07-13) must have NO live occurrence left.
    expect(
      out.find((o) => !o.cancelled && o.start.startsWith('2026-07-13')),
    ).toBeUndefined();
  });

  it("suppresses Lea's exact Google series shape (summer DTSTART + summer cancels)", () => {
    // The actual data from Lea's log: master 5nqo9avp8 — weekly Monday, tzid
    // Europe/Berlin, DTSTART 2025-08-11 10:30Z (= Mon 12:30 CEST), with cancelled
    // overrides on 2026-07-06 and 2026-07-13 (both 10:30Z). All three land at
    // 10:30Z, so suppression must drop BOTH Mondays.
    const master = mkEvent({
      id: '5nqo9avp8',
      start: '2025-08-11T10:30:00.000Z',
      end: '2025-08-11T11:00:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=MO',
        exceptions: [],
        tzid: 'Europe/Berlin',
      },
    });
    const cancel06 = mkEvent({
      id: '5nqo9avp8::rid::2026-07-06T10:30:00Z',
      start: '2026-07-06T10:30:00.000Z',
      end: '2026-07-06T10:30:00.000Z',
      cancelled: true,
    });
    const cancel13 = mkEvent({
      id: '5nqo9avp8::rid::2026-07-13T10:30:00Z',
      start: '2026-07-13T10:30:00.000Z',
      end: '2026-07-13T10:30:00.000Z',
      cancelled: true,
    });
    const out = expandAll([master, cancel06, cancel13], {
      start: new Date('2026-07-01T00:00:00Z'),
      end: new Date('2026-07-31T00:00:00Z'),
    });
    const liveMondays = out.filter(
      (o) =>
        !o.cancelled &&
        (o.start.startsWith('2026-07-06') || o.start.startsWith('2026-07-13')),
    );
    expect(liveMondays).toEqual([]);
  });
});

describe('expandEvent timezone (DST-correct)', () => {
  /** The local calendar day of an instant in a given IANA zone — system-TZ
   *  independent (uses Intl with an explicit timeZone). */
  const dayIn = (iso: string, tz: string) =>
    new Intl.DateTimeFormat('en-CA', { timeZone: tz }).format(new Date(iso));

  it('keeps a zoned monthly series on the right local day + time across DST (oagdu)', () => {
    // 2nd Sunday monthly, 19:00 America/New_York, authored in winter (EST):
    // 2025-12-14 19:00 EST = 2025-12-15T00:00:00Z.
    const ev = mkEvent({
      id: 'oagdu',
      start: '2025-12-15T00:00:00.000Z',
      end: '2025-12-15T01:00:00.000Z',
      recurrence: {
        rrule: 'FREQ=MONTHLY;BYDAY=2SU',
        exceptions: [],
        tzid: 'America/New_York',
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-07-01T00:00:00Z'),
      end: new Date('2026-08-01T00:00:00Z'),
    });
    expect(out.length).toBe(1);
    // 19:00 EDT (UTC-4) on Sunday the 12th = 23:00Z — NOT 00:00Z (which is
    // Saturday the 11th, the pre-fix bug that hid the event from Sunday).
    expect(out[0].start).toBe('2026-07-12T23:00:00.000Z');
    expect(dayIn(out[0].start, 'America/New_York')).toBe('2026-07-12');
  });

  it('preserves wall-clock time across the spring-forward boundary', () => {
    // Weekly Monday 09:00 America/New_York, dtstart in winter (09:00 EST = 14:00Z).
    const ev = mkEvent({
      id: 'wk',
      start: '2026-02-02T14:00:00.000Z',
      end: '2026-02-02T14:30:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=MO',
        exceptions: [],
        tzid: 'America/New_York',
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-03-01T00:00:00Z'),
      end: new Date('2026-03-16T00:00:00Z'),
    });
    const starts = out.map((o) => o.start);
    // Mar 2 is still EST (09:00 = 14:00Z); Mar 9 is EDT after the Mar 8 change
    // (09:00 = 13:00Z, NOT 14:00Z which would be 10:00 EDT — the drift bug).
    expect(starts).toContain('2026-03-02T14:00:00.000Z');
    expect(starts).toContain('2026-03-09T13:00:00.000Z');
    // Every occurrence stays on Monday 09:00 local.
    out.forEach((o) =>
      expect(
        new Intl.DateTimeFormat('en-US', {
          timeZone: 'America/New_York',
          hour: '2-digit',
          hourCycle: 'h23',
        }).format(new Date(o.start)),
      ).toBe('09'),
    );
  });

  it('falls back to UTC expansion for an unresolvable tzid (no crash)', () => {
    const ev = mkEvent({
      id: 'bad',
      start: '2026-05-19T09:00:00.000Z',
      end: '2026-05-19T09:30:00.000Z',
      recurrence: {
        rrule: 'FREQ=DAILY;COUNT=3',
        exceptions: [],
        tzid: 'Not/ARealZone',
      },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    // Degrades to the UTC path — still produces all occurrences, never drops.
    expect(out.length).toBe(3);
    expect(out[0].start).toBe('2026-05-19T09:00:00.000Z');
  });

  it('leaves a tzid-less recurring event on the unchanged UTC path', () => {
    const zoned = mkEvent({
      id: 'z',
      start: '2026-05-19T09:00:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY;COUNT=2', exceptions: [] },
    });
    const out = expandEvent(zoned, {
      start: new Date('2026-05-19'),
      end: new Date('2026-05-30'),
    });
    expect(out.map((o) => o.start)).toEqual([
      '2026-05-19T09:00:00.000Z',
      '2026-05-20T09:00:00.000Z',
    ]);
  });

  it('rounds a spring-forward gap occurrence forward to the first valid time', () => {
    // DAILY 02:30 America/New_York across 2026-03-08 (02:00 EST jumps to 03:00
    // EDT, so 02:30 does not exist that day). dtstart 02:30 EST = 07:30Z.
    const ev = mkEvent({
      id: 'gap',
      start: '2026-03-01T07:30:00.000Z',
      end: '2026-03-01T08:00:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY', exceptions: [], tzid: 'America/New_York' },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-03-07T00:00:00Z'),
      end: new Date('2026-03-10T00:00:00Z'),
    });
    const onGapDay = out.find(
      (o) => dayIn(o.start, 'America/New_York') === '2026-03-08',
    );
    // Rounds forward to 03:30 EDT = 07:30Z (not 01:30 EST, the old behaviour).
    expect(onGapDay?.start).toBe('2026-03-08T07:30:00.000Z');
  });

  it('takes the first reading of a fall-back ambiguous occurrence', () => {
    // DAILY 01:30 America/New_York across 2026-11-01 (02:00 EDT falls back to
    // 01:00 EST, so 01:30 happens twice). dtstart 01:30 EDT = 05:30Z.
    const ev = mkEvent({
      id: 'overlap',
      start: '2026-10-01T05:30:00.000Z',
      end: '2026-10-01T06:00:00.000Z',
      recurrence: { rrule: 'FREQ=DAILY', exceptions: [], tzid: 'America/New_York' },
    });
    const out = expandEvent(ev, {
      start: new Date('2026-10-31T00:00:00Z'),
      end: new Date('2026-11-03T00:00:00Z'),
    });
    const onFallDay = out.find(
      (o) => dayIn(o.start, 'America/New_York') === '2026-11-01',
    );
    // The FIRST 01:30 (EDT) = 05:30Z, not the second 01:30 (EST) = 06:30Z.
    expect(onFallDay?.start).toBe('2026-11-01T05:30:00.000Z');
  });
});

describe('withCreatedRecurrenceZone', () => {
  const rule: { rrule: string; exceptions: string[]; tzid?: string | null } = {
    rrule: 'FREQ=WEEKLY',
    exceptions: [],
  };

  it('stamps the host zone onto a new timed recurring rule', () => {
    const out = withCreatedRecurrenceZone(rule, false);
    // localTimeZone() is null on a UTC host (then no stamp); equal otherwise.
    expect(out?.tzid ?? null).toBe(localTimeZone());
    expect(rule.tzid).toBeUndefined(); // input untouched
  });

  it('leaves an all-day rule unzoned (it uses the date-based path)', () => {
    expect(withCreatedRecurrenceZone(rule, true)?.tzid).toBeUndefined();
  });

  it('keeps an already-zoned rule exactly as-is', () => {
    const zoned = { rrule: 'FREQ=WEEKLY', exceptions: [], tzid: 'America/New_York' };
    expect(withCreatedRecurrenceZone(zoned, false)).toBe(zoned);
  });

  it('passes a non-recurring event (null) through', () => {
    expect(withCreatedRecurrenceZone(null, false)).toBeNull();
  });
});

describe('truncateRRuleBefore', () => {
  it('sets UNTIL one second before the cutoff, keeping earlier occurrences', () => {
    const out = truncateRRuleBefore(
      'FREQ=WEEKLY;INTERVAL=2;BYDAY=TH',
      new Date('2026-08-06T12:00:00.000Z'),
    );
    expect(out).toBe('FREQ=WEEKLY;INTERVAL=2;BYDAY=TH;UNTIL=20260806T115959Z');
  });

  it('drops COUNT (a rule cannot carry both COUNT and UNTIL)', () => {
    const out = truncateRRuleBefore(
      'FREQ=DAILY;COUNT=10',
      new Date('2026-05-25T09:00:00.000Z'),
    );
    expect(out).toBe('FREQ=DAILY;UNTIL=20260525T085959Z');
  });

  it('never extends a series: an earlier existing UNTIL wins', () => {
    const out = truncateRRuleBefore(
      'FREQ=DAILY;UNTIL=20260510T090000Z',
      new Date('2026-08-06T12:00:00.000Z'),
    );
    expect(out).toBe('FREQ=DAILY;UNTIL=20260510T090000Z');
  });

  it('tolerates a leading RRULE: prefix', () => {
    const out = truncateRRuleBefore(
      'RRULE:FREQ=DAILY',
      new Date('2026-05-25T09:00:00.000Z'),
    );
    expect(out).toBe('FREQ=DAILY;UNTIL=20260525T085959Z');
  });

  it('truncates a series so the cutoff occurrence and all after it fall away', () => {
    const master = mkEvent({
      start: '2026-07-06T09:00:00.000Z',
      end: '2026-07-06T09:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], tzid: null },
    });
    const cutoff = new Date('2026-07-20T09:00:00.000Z'); // the 3rd Monday
    const truncated = {
      ...master,
      recurrence: {
        ...master.recurrence!,
        rrule: truncateRRuleBefore(master.recurrence!.rrule, cutoff),
      },
    };
    const starts = expandEvent(truncated, {
      start: new Date('2026-07-01T00:00:00.000Z'),
      end: new Date('2026-09-01T00:00:00.000Z'),
    }).map((o) => o.start);
    expect(starts).toEqual([
      '2026-07-06T09:00:00.000Z',
      '2026-07-13T09:00:00.000Z',
    ]);
  });
});

describe('splitRRuleForEdit', () => {
  const cutoff = new Date('2026-08-06T12:00:00.000Z');

  it('open-ended series: old truncated, new keeps the same open rule', () => {
    const { oldRule, newRule } = splitRRuleForEdit(
      'FREQ=WEEKLY;INTERVAL=2;BYDAY=TH',
      cutoff,
      3,
    );
    expect(oldRule).toBe('FREQ=WEEKLY;INTERVAL=2;BYDAY=TH;UNTIL=20260806T115959Z');
    expect(newRule).toBe('FREQ=WEEKLY;INTERVAL=2;BYDAY=TH');
  });

  it('UNTIL series: the new series carries the same absolute UNTIL', () => {
    const { newRule } = splitRRuleForEdit(
      'FREQ=DAILY;UNTIL=20261231T235959Z',
      cutoff,
      5,
    );
    expect(newRule).toBe('FREQ=DAILY;UNTIL=20261231T235959Z');
  });

  it('COUNT series: the new series gets the REMAINING count', () => {
    const { oldRule, newRule } = splitRRuleForEdit(
      'FREQ=WEEKLY;BYDAY=TH;COUNT=10',
      cutoff,
      3, // three occurrences kept on the old series
    );
    // old series drops COUNT for a hard UNTIL cutoff…
    expect(oldRule).toBe('FREQ=WEEKLY;BYDAY=TH;UNTIL=20260806T115959Z');
    // …the new series continues for the remaining 7.
    expect(newRule).toBe('FREQ=WEEKLY;BYDAY=TH;COUNT=7');
  });

  it('COUNT never drops below 1', () => {
    const { newRule } = splitRRuleForEdit('FREQ=DAILY;COUNT=3', cutoff, 5);
    expect(newRule).toBe('FREQ=DAILY;COUNT=1');
  });
});
