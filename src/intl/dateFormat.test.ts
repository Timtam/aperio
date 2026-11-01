import { describe, expect, it } from 'vitest';
import { localeFor } from './dateFormat';
import { format } from 'date-fns';
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
