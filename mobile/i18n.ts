import { getLocales } from 'expo-localization';
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import de from '@aperio/locales/de/translation.json';
import en from '@aperio/locales/en/translation.json';

// The mobile app reuses the desktop's translation files (the shared
// @aperio/locales package) so there is one source of truth for every string —
// the UI is rebuilt per platform, the wording is not. Mobile-only strings live
// under the `mobile.*` key in those same files.

/** The languages Aperio ships translations for (matches the desktop). */
export const SUPPORTED_LANGUAGES = ['de', 'en'] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

/**
 * Best-effort device language. Unlike the desktop (which reads the
 * preference-ordered `navigator.languages` from the Tauri webview), React
 * Native's Hermes engine does NOT reflect the OS language list through `Intl`
 * (on iOS it resolves to en-US regardless of the device setting), so we read the
 * OS preference list from expo-localization instead. `getLocales()` is ordered
 * by the user's preference and guaranteed non-empty; we return the first locale
 * whose language is one we ship, else fall back to English.
 */
export function detectSystemLanguage(): SupportedLanguage {
  try {
    for (const locale of getLocales()) {
      const base = (locale.languageCode ?? locale.languageTag)
        ?.toLowerCase()
        .split(/[-_]/)[0];
      if (base === 'de') return 'de';
      if (base === 'en') return 'en';
    }
  } catch {
    // expo-localization unavailable — fall through to the default.
  }
  return 'en';
}

void i18n.use(initReactI18next).init({
  resources: {
    de: { translation: de },
    en: { translation: en },
  },
  lng: detectSystemLanguage(),
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
});

export default i18n;
