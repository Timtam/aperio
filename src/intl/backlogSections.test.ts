import { describe, expect, it } from 'vitest';

import { backlogWeeks } from '@aperio/shared';
import i18n from '../i18n';
import { backlogSectionNames } from './backlogSections';

/**
 * The backlog's section headings (TODO B15 step 5): the week for the eye and
 * the ear, and the count with its noun when heard, not a number in brackets.
 */

const de = i18n.getFixedT('de');
const en = i18n.getFixedT('en');

describe('the backlog sections', () => {
  // Wednesday, 4 November 2026: this week is 45, next week 46.
  const weeks = backlogWeeks('2026-11-04', 1);
  const counts = { thisWeek: 3, nextWeek: 1, rest: 0 };

  it('name this week and next week, seen and heard', () => {
    const names = backlogSectionNames(de, weeks, 1, true, counts);
    expect(names.thisWeek).toEqual({
      heading: 'Diese Woche (3) · KW 45',
      spoken: 'Diese Woche, Kalenderwoche 45, 3 Aufgaben',
    });
    expect(names.nextWeek).toEqual({
      heading: 'Nächste Woche (1) · KW 46',
      spoken: 'Nächste Woche, Kalenderwoche 46, 1 Aufgabe',
    });
  });

  it('name no week for the rest, which spans none', () => {
    expect(backlogSectionNames(de, weeks, 1, true, counts).rest).toEqual({
      heading: 'Weiteres (0)',
      spoken: 'Weiteres, 0 Aufgaben',
    });
  });

  it('leave the week out with weeks switched off', () => {
    expect(backlogSectionNames(de, weeks, 1, false, counts).thisWeek).toEqual({
      heading: 'Diese Woche (3)',
      spoken: 'Diese Woche, 3 Aufgaben',
    });
  });

  it('follow the week start, and the turn of the year', () => {
    // Sunday start: Sunday 1 November opens week 45.
    expect(
      backlogSectionNames(de, backlogWeeks('2026-11-01', 0), 0, true, counts).thisWeek.spoken,
    ).toBe('Diese Woche, Kalenderwoche 45, 3 Aufgaben');
    expect(
      backlogSectionNames(de, backlogWeeks('2026-12-30', 1), 1, true, counts).thisWeek,
    ).toEqual({
      heading: 'Diese Woche (3) · KW 53 (2026)',
      spoken: 'Diese Woche, Kalenderwoche 53 des Jahres 2026, 3 Aufgaben',
    });
  });

  it('in English', () => {
    expect(backlogSectionNames(en, weeks, 1, true, counts).nextWeek).toEqual({
      heading: 'Next week (1) · Week 46',
      spoken: 'Next week, Week 46, 1 task',
    });
  });
});
