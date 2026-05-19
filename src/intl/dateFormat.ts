import {
  format as dfFormat,
  formatDistanceToNow as dfFormatDistanceToNow,
  getISOWeek as dfGetISOWeek,
  type Locale,
} from 'date-fns';
import { de as deLocale } from 'date-fns/locale/de';
import { enUS as enLocale } from 'date-fns/locale/en-US';
import { useTranslation } from 'react-i18next';
import { useMemo } from 'react';

/**
 * date-fns wiring keyed off the active i18next language.
 *
 * The app uses ISO-8601 calendar weeks throughout (DESIGN.md section 5.2)
 * — Monday-start, the first week of a year being the one with at least
 * four days in that year. `date-fns`' `getISOWeek` implements this; we
 * re-export it from here so the rest of the app doesn't have to know
 * about date-fns directly.
 */

const LOCALES: Record<string, Locale> = {
  de: deLocale,
  en: enLocale,
};

export function localeFor(language: string): Locale {
  // Strip the region (e.g. `de-DE` → `de`). i18next does this internally
  // for us already, but be defensive.
  const base = language.split(/[-_]/, 1)[0];
  return LOCALES[base] ?? enLocale;
}

export interface DateFormatter {
  format: (date: Date | number, pattern: string) => string;
  formatRelative: (date: Date | number) => string;
  isoWeek: (date: Date | number) => number;
  locale: Locale;
}

/**
 * Hook: returns a stable formatter bound to the current i18next language.
 *
 * Use `formatter.format(date, 'PPpp')` instead of the global `format()`
 * from `date-fns` to get locale-correct output everywhere.
 */
export function useDateFormat(): DateFormatter {
  const { i18n } = useTranslation();
  return useMemo<DateFormatter>(() => {
    const locale = localeFor(i18n.language);
    return {
      locale,
      format: (date, pattern) => dfFormat(date, pattern, { locale }),
      formatRelative: (date) =>
        dfFormatDistanceToNow(date, { locale, addSuffix: true }),
      isoWeek: (date) => dfGetISOWeek(date),
    };
  }, [i18n.language]);
}
