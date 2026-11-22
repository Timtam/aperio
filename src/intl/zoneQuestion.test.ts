import { describe, expect, it } from 'vitest';

import {
  fieldsAt,
  seriesClockChange,
  weekdayQuestion,
  zoneAppliedSeries,
  zoneChoiceAnnouncement,
  zoneChoiceEntry,
  zoneOffsets,
  zoneRefusedAnnouncement,
  zoneUnchangedAnnouncement,
} from '@aperio/shared';
import i18n from '../i18n';

/**
 * What choosing a series' zone says (stage 12 of DESIGN-series-time-zone.md,
 * 138a): the weekday question, the announcements after a choice, and the
 * series a whole-series save measures against after an answer. In German.
 */

const de = i18n.getFixedT('de');
const en = i18n.getFixedT('en');
const BERLIN = 'Europe/Berlin';
/** Sunday 22 November 2026, 23:30 UTC: Monday 00:30 in Berlin. */
const SUNDAY_LATE = '2026-11-22T23:30:00.000Z';

const askAbout = (rrule: string) => {
  const change = seriesClockChange({
    rrule,
    start: SUNDAY_LATE,
    exceptions: [],
    fromTzid: null,
    toTzid: BERLIN,
  });
  if (change === null) throw new Error('the core gave no answer');
  return change;
};

describe('the weekday question (138a)', () => {
  it('words a series that really means UTC: the rule moves the events a day earlier', () => {
    // Sundays and Wednesdays at 23:30 UTC. A rule naming the start's day
    // alone is not asked about (24a); this one names another day too.
    const change = askAbout('FREQ=WEEKLY;BYDAY=SU,WE');
    expect(change.asks).toBe(true);
    const question = weekdayQuestion({
      change,
      start: SUNDAY_LATE,
      rrule: 'FREQ=WEEKLY;BYDAY=SU,WE',
      zone: BERLIN,
      language: 'de',
      t: de,
    });
    expect(question).toEqual({
      title: 'Welcher Wochentag soll gelten?',
      message: 'In Berlin fällt der Beginn auf Montag, 23. November, 00:30; die Regel sagt jeden Mittwoch und Sonntag.',
      keep: 'Zeitzone nicht ändern',
      answers: [
        { answer: 'rule', label: 'Jeden Mittwoch und Sonntag, wie die Regel sagt (Termine rücken einen Tag vor)' },
        { answer: 'now', label: 'Jeden Montag und Donnerstag, wo die Termine jetzt liegen' },
      ],
    });
  });

  it('words a series that lost its Berlin zone: the rule keeps the start', () => {
    const change = askAbout('FREQ=WEEKLY;BYDAY=MO');
    const question = weekdayQuestion({
      change,
      start: SUNDAY_LATE,
      rrule: 'FREQ=WEEKLY;BYDAY=MO',
      zone: BERLIN,
      language: 'de',
      t: de,
    });
    // The start stays, but the events move: from Tuesday 00:30 in Berlin,
    // where they fall now, back to Monday, where the rule meant them.
    expect(question.answers).toEqual([
      { answer: 'rule', label: 'Jeden Montag, wie die Regel sagt (Termine rücken einen Tag vor)' },
      { answer: 'now', label: 'Jeden Dienstag, wo die Termine jetzt liegen' },
    ]);
  });

  it('leaves out an answer that cannot be carried out, and says why', () => {
    const change = askAbout('FREQ=WEEKLY;BYDAY=SU,WE');
    const question = weekdayQuestion({
      change: { ...change, asNow: { outcome: 'refused', reason: 'skipped_time' } },
      start: SUNDAY_LATE,
      rrule: 'FREQ=WEEKLY;BYDAY=SU,WE',
      zone: BERLIN,
      language: 'de',
      t: de,
    });
    expect(question.answers.map((answer) => answer.answer)).toEqual(['rule']);
    expect(question.message).toBe(
      'In Berlin fällt der Beginn auf Montag, 23. November, 00:30; die Regel sagt jeden Mittwoch und Sonntag. ' +
        'Die Tage, wo die Termine jetzt liegen, gehen nicht: der Beginn oder ein gelöschter Termin fiele in die Stunde, die die Uhren in Berlin überspringen.',
    );
  });

  it('reads a day the rule takes from its start on the old clock, and keeps one period', () => {
    // Monthly from 31 January 23:30 UTC: on Berlin's clock 1 February.
    const start = '2026-01-31T23:30:00.000Z';
    const change = seriesClockChange({ rrule: 'FREQ=MONTHLY', start, exceptions: [], fromTzid: null, toTzid: BERLIN });
    if (change === null || !change.asks) throw new Error('expected the core to ask');
    const question = weekdayQuestion({
      change,
      start,
      rrule: 'FREQ=MONTHLY',
      zone: BERLIN,
      language: 'de',
      t: de,
    });
    expect(question.message).not.toMatch(/am 1\./);
    expect(question.message).not.toMatch(/\.\.$/);
    expect(question.message.startsWith('In Berlin fällt der Beginn auf Sonntag, 1. Februar, 00:30; die Regel sagt ')).toBe(true);
  });

  it('says the events move a day later for a westward repair', () => {
    // A New York series that lost its zone: Sundays at 01:30 UTC, which is
    // Saturday evening in New York. As the rule says, Sunday evening there.
    const start = '2026-11-23T01:30:00.000Z';
    const change = seriesClockChange({
      rrule: 'FREQ=WEEKLY;BYDAY=SU',
      start,
      exceptions: [],
      fromTzid: null,
      toTzid: 'America/New_York',
    });
    if (change === null || !change.asks) throw new Error('expected the core to ask');
    const question = weekdayQuestion({
      change,
      start,
      rrule: 'FREQ=WEEKLY;BYDAY=SU',
      zone: 'America/New_York',
      language: 'de',
      t: de,
    });
    expect(question.answers.find((answer) => answer.answer === 'rule')?.label).toBe(
      'Jeden Sonntag, wie die Regel sagt (Termine rücken einen Tag zurück)',
    );
  });

  it('says the events move where both start days are rule days', () => {
    const change = askAbout('FREQ=WEEKLY;BYDAY=SU,MO');
    if (!change.asks) throw new Error('expected the core to ask');
    const question = weekdayQuestion({
      change,
      start: SUNDAY_LATE,
      rrule: 'FREQ=WEEKLY;BYDAY=SU,MO',
      zone: BERLIN,
      language: 'de',
      t: de,
    });
    expect(question.answers.find((answer) => answer.answer === 'rule')?.label).toMatch(
      /\(Termine rücken einen Tag vor\)$/,
    );
  });

  it('says the same in English', () => {
    const change = askAbout('FREQ=WEEKLY;BYDAY=SU,WE');
    const question = weekdayQuestion({
      change,
      start: SUNDAY_LATE,
      rrule: 'FREQ=WEEKLY;BYDAY=SU,WE',
      zone: BERLIN,
      language: 'en',
      t: en,
    });
    expect(question.title).toBe('Which weekday should apply?');
    expect(question.keep).toBe("Don't change the time zone");
    expect(question.answers[0].label).toMatch(/, as the rule says \(events move one day earlier\)$/);
  });
});

describe('what a choice says', () => {
  const NEW_YORK = { kind: 'series', tzid: 'America/New_York' } as const;
  /** Monday 14 September 2026, 09:00 to 10:00 in New York. */
  const fields = fieldsAt('2026-09-14T13:00:00.000Z', '2026-09-14T14:00:00.000Z', NEW_YORK);

  it('keeps the typed times of a new event, on the new clock', () => {
    expect(
      zoneChoiceAnnouncement({ kind: 'typed', zone: 'America/New_York', fields, rewritten: null, language: 'de', t: de }),
    ).toBe(
      'Zeitzone der Serie: New York. Die Uhrzeiten bleiben wie getippt und gelten jetzt auf dieser Uhr: Montag, 14. September, 09:00 bis 10:00.',
    );
  });

  it('keeps the instant of something stored', () => {
    expect(
      zoneChoiceAnnouncement({
        kind: 'sameInstant',
        zone: 'America/New_York',
        fields,
        rewritten: null,
        language: 'de',
        t: de,
      }),
    ).toBe(
      'Zeitzone der Serie: New York. Die Serie beginnt zum selben Zeitpunkt: Montag, 14. September, 09:00 bis 10:00 auf dieser Uhr.',
    );
  });

  it('says a moved start, and the days of a rewritten rule', () => {
    expect(
      zoneChoiceAnnouncement({
        kind: 'startMoved',
        zone: 'America/New_York',
        fields,
        rewritten: 'FREQ=WEEKLY;BYDAY=MO',
        language: 'de',
        t: de,
      }),
    ).toBe(
      'Zeitzone der Serie: New York. Die Serie beginnt jetzt am Montag, 14. September, 09:00 auf dieser Uhr. Wiederholt sich jeden Montag.',
    );
  });

  it('names the zone in effect, unchanged, and a change that cannot be carried out', () => {
    const offsets = zoneOffsets('2026-11-22T12:00:00Z', '2026-11-22T12:00:00Z');
    expect(zoneChoiceEntry(BERLIN, offsets, de)).toBe('Berlin, Europa, UTC+01:00');
    expect(zoneChoiceEntry('Europe/Kiev', offsets, de)).toBe('Europe/Kiev (nicht in Aperios Liste)');
    expect(zoneChoiceEntry(null, offsets, de)).toBe('UTC (ohne Sommerzeit)');
    expect(zoneUnchangedAnnouncement('UTC (ohne Sommerzeit)', de)).toBe(
      'Zeitzone der Serie nicht geändert: UTC (ohne Sommerzeit)',
    );
    expect(zoneRefusedAnnouncement('time_of_day', BERLIN, de)).toBe(
      'Zeitzone der Serie nicht geändert: ihre Regel legt eigene Uhrzeiten fest.',
    );
  });
});

describe('the series after an answer (question 12)', () => {
  it('starts where the answer put it, keeps its length, and carries the rule, the exceptions and the zone', () => {
    const series = {
      id: 's',
      start: SUNDAY_LATE,
      end: '2026-11-23T00:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=SU', exceptions: [], tzid: null },
    };
    const applied = zoneAppliedSeries(
      series,
      { start: '2026-11-21T23:30:00.000Z', rrule: 'FREQ=WEEKLY;BYDAY=SU', exceptions: ['2026-11-28T23:30:00.000Z'] },
      BERLIN,
    );
    expect(applied).toEqual({
      id: 's',
      start: '2026-11-21T23:30:00.000Z',
      end: '2026-11-22T00:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=SU', exceptions: ['2026-11-28T23:30:00.000Z'], tzid: BERLIN },
    });
    expect(zoneAppliedSeries(series, { start: SUNDAY_LATE, rrule: 'FREQ=DAILY', exceptions: [] }, null).recurrence?.tzid).toBeNull();
  });
});
