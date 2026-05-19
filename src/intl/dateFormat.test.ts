import { describe, expect, it } from 'vitest';
import { localeFor } from './dateFormat';
import { getISOWeek, format } from 'date-fns';
import { de as deLocale } from 'date-fns/locale/de';
import { enUS as enLocale } from 'date-fns/locale/en-US';

describe('localeFor', () => {
  it('returns the German locale for de / de-DE', () => {
    expect(localeFor('de')).toBe(deLocale);
    expect(localeFor('de-DE')).toBe(deLocale);
  });

  it('falls back to en-US for unknown languages', () => {
    expect(localeFor('zz')).toBe(enLocale);
  });
});

describe('ISO-8601 week numbers', () => {
  // Anchor: 2024-12-30 is in ISO week 1 of 2025.
  it('places 2024-12-30 in ISO week 1', () => {
    expect(getISOWeek(new Date(2024, 11, 30))).toBe(1);
  });

  it('places 2025-01-01 in ISO week 1', () => {
    expect(getISOWeek(new Date(2025, 0, 1))).toBe(1);
  });

  it('places 2026-12-31 in ISO week 53', () => {
    expect(getISOWeek(new Date(2026, 11, 31))).toBe(53);
  });
});

describe('locale-aware formatting', () => {
  it('produces German month names with the de locale', () => {
    const d = new Date(2025, 4, 19);
    expect(format(d, 'd. MMMM yyyy', { locale: deLocale })).toBe(
      '19. Mai 2025',
    );
  });

  it('produces English month names with the en-US locale', () => {
    const d = new Date(2025, 4, 19);
    expect(format(d, 'MMMM d, yyyy', { locale: enLocale })).toBe('May 19, 2025');
  });
});
