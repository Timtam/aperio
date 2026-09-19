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
    t: (key, values) => t(key, values as never) as string,
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

  it('says what it cannot put into words, and how often that rule repeats', () => {
    const row = rows.find((r) => r.name === 'monthly_plain_byday');
    expect(row).toBeDefined();
    // Not a guessed sentence, and not silence.
    expect(sentence(row!, 'de')).toMatch(/monatlich/);
    expect(sentence(row!, 'en')).toMatch(/monthly/);
  });
});
