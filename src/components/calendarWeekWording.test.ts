import { describe, expect, it } from 'vitest';

import {
  calendarWeekLabel,
  calendarWeekName,
  calendarWeekOf,
  calendarWeekRange,
  calendarWeekSentence,
} from '@aperio/shared';
import i18n from '../i18n';

/**
 * How a week is written, the same words on both devices
 * (`shared/calendarWeek.ts`). Spoken it says "Kalenderwoche", never "KW",
 * which a synthesizer spells out; a range says "bis", never a dash; the
 * week's number is never followed by a full stop, which would be read as an
 * ordinal.
 */

const de = i18n.getFixedT('de');
const en = i18n.getFixedT('en');

// Monday to Sunday in one month.
const oneMonth = calendarWeekOf('2026-11-04', 1);
// Monday, 26 October, to Sunday, 1 November.
const twoMonths = calendarWeekOf('2026-10-28', 1);
// Monday, 28 December 2026, to Sunday, 3 January 2027: week 53 of 2026.
const twoYears = calendarWeekOf('2027-01-01', 1);

describe('a week, written', () => {
  it('spoken: the word in full, the year only across a turn of the year', () => {
    expect(calendarWeekName(de, oneMonth)).toBe('Kalenderwoche 45');
    expect(calendarWeekName(de, twoYears)).toBe('Kalenderwoche 53 des Jahres 2026');
    expect(calendarWeekName(en, oneMonth)).toBe('Week 45');
    expect(calendarWeekName(en, twoYears)).toBe('Week 53 of 2026');
  });

  it('seen: the short form, the year in brackets across a turn of the year', () => {
    expect(calendarWeekLabel(de, oneMonth)).toBe('KW 45');
    expect(calendarWeekLabel(de, twoYears)).toBe('KW 53 (2026)');
    expect(calendarWeekLabel(en, twoYears)).toBe('Week 53 (2026)');
  });

  it('its days with "bis", naming a month or a year once where they are shared', () => {
    expect(calendarWeekRange(de, 'de', oneMonth)).toBe('2. bis 8. November 2026');
    expect(calendarWeekRange(de, 'de', twoMonths)).toBe('26. Oktober bis 1. November 2026');
    expect(calendarWeekRange(de, 'de', twoYears)).toBe(
      '28. Dezember 2026 bis 3. Januar 2027',
    );
    expect(calendarWeekRange(en, 'en', oneMonth)).toBe('November 2 to 8, 2026');
    expect(calendarWeekRange(en, 'en', twoMonths)).toBe('October 26 to November 1, 2026');
    expect(calendarWeekRange(en, 'en', twoYears)).toBe(
      'December 28, 2026 to January 3, 2027',
    );
  });

  it('in one sentence, what the K key says', () => {
    expect(calendarWeekSentence(de, 'de', oneMonth)).toBe(
      'Kalenderwoche 45, 2. bis 8. November 2026',
    );
    expect(calendarWeekSentence(de, 'de', twoYears)).toBe(
      'Kalenderwoche 53 des Jahres 2026, 28. Dezember 2026 bis 3. Januar 2027',
    );
    expect(calendarWeekSentence(en, 'en', oneMonth)).toBe('Week 45, November 2 to 8, 2026');
  });

  it('follows the week start: Sunday to Saturday is one week with one number', () => {
    const sundayStart = calendarWeekOf('2026-11-01', 0);
    expect(calendarWeekSentence(de, 'de', sundayStart)).toBe(
      'Kalenderwoche 45, 1. bis 7. November 2026',
    );
  });
});
