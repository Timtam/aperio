import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/recurrenceSummary.json';
import {
  describeRecurrence,
  recurrenceSummaryText,
  type RecurrenceSummary,
} from '@aperio/shared';
import i18n from '../i18n';

/**
 * A repeat rule in words (decision 84a), row by row through the REAL door.
 *
 * The core decides what the sentence says; this surface looks the words up.
 * So each row is asked of `cal_core::recurrence_summary` in WebAssembly — the
 * same answer the phone gets through its own door — and then rendered against
 * the locale files the app ships. A wording that drifts in either language,
 * or a shape the core stops describing, fails here.
 */

interface Row {
  name: string;
  rrule: string;
  start: string;
  last_day?: string;
  expected: unknown;
  en: string;
  de: string;
}

const rows = contract.rows as unknown as Row[];

const sentence = (row: Row, language: 'en' | 'de'): string => {
  const summary = describeRecurrence({
    rrule: row.rrule,
    start: row.start,
    last_day: row.last_day ?? null,
  }) as RecurrenceSummary;
  expect(summary).toEqual(row.expected);
  const t = i18n.getFixedT(language);
  return recurrenceSummaryText(summary, {
    t: (key, values) => t(key, values as never) as unknown as string,
    language,
  });
};

describe('a repeat rule in words', () => {
  it('covers the shapes a provider writes', () => {
    expect(rows.length).toBeGreaterThanOrEqual(40);
  });

  for (const row of rows) {
    it(`${row.name}: ${row.en || 'no repeat'}`, () => {
      expect(sentence(row, 'en')).toBe(row.en);
      expect(sentence(row, 'de')).toBe(row.de);
    });
  }

  it('has a sentence for every key the core emits, in both languages', () => {
    // The core proves every key it can emit appears in a row (its own
    // `every_key_the_core_emits_is_used_by_a_row`), so the rows are the whole
    // set — and a key without a sentence here is a silent gap in the app.
    const keys = new Set<string>();
    for (const row of rows) {
      const expected = row.expected as {
        key?: string;
        every?: { key: string };
        on?: { key: string };
        end?: { key: string };
      };
      for (const key of [expected.key, expected.every?.key, expected.on?.key, expected.end?.key]) {
        if (key !== undefined) keys.add(key);
      }
    }
    // The renderer's own keys too: the core names a weekday, an ordinal or a
    // set, and these are the words that come out of that. A missing one would
    // read as the key itself.
    for (const key of [
      'recurrenceSummary.set.weekday',
      'recurrenceSummary.set.weekendDay',
      'recurrenceSummary.ordinal.1',
      'recurrenceSummary.ordinal.2',
      'recurrenceSummary.ordinal.3',
      'recurrenceSummary.ordinal.4',
      'recurrenceSummary.ordinal.5',
      'recurrenceSummary.ordinal.last',
      'recurrenceSummary.day.number',
      'recurrenceSummary.list.separator',
      'recurrenceSummary.list.and',
      'recurrenceSummary.undescribed.known',
      'recurrenceSummary.undescribed.unknown',
      'recurrenceSummary.unitAdverb.second',
      'recurrenceSummary.unitAdverb.minute',
      'recurrenceSummary.unitAdverb.hour',
      'recurrenceSummary.unitAdverb.day',
      'recurrenceSummary.unitAdverb.week',
      'recurrenceSummary.unitAdverb.month',
      'recurrenceSummary.unitAdverb.year',
    ]) {
      keys.add(key);
    }
    expect(keys.size).toBeGreaterThan(20);
    for (const language of ['en', 'de'] as const) {
      const t = i18n.getFixedT(language);
      for (const key of keys) {
        const sentence = t(key, { count: 2 }) as unknown as string;
        expect(sentence, `${language} has no sentence for ${key}`).not.toBe(key);
        expect(sentence.trim()).not.toBe('');
      }
    }
  });

  it('says what it cannot put into words, and how often that rule repeats', () => {
    const row = rows.find((r) => r.name === 'monthly_plain_byday');
    expect(row).toBeDefined();
    // Not a guessed sentence, and not silence.
    expect(sentence(row!, 'de')).toMatch(/monatlich/);
    expect(sentence(row!, 'en')).toMatch(/monthly/);
  });
});
