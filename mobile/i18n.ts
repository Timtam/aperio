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
 * Best-effort device language. Unlike the desktop (which reads
 * `navigator.languages` from the Tauri webview), React Native has no such API,
 * so we use the locale the JS engine's `Intl` resolves from the OS. Falls back
 * to English when neither shipped language matches.
 */
export function detectSystemLanguage(): SupportedLanguage {
  try {
    const locale = Intl.DateTimeFormat().resolvedOptions().locale;
    const base = locale.toLowerCase().split(/[-_]/)[0];
    if (base === 'de') return 'de';
    if (base === 'en') return 'en';
  } catch {
    // Intl unavailable — fall through to the default.
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
