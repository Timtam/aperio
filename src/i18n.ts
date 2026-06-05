import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import de from './locales/de/translation.json';
import en from './locales/en/translation.json';

/** The languages Aperio ships translations for. */
export const SUPPORTED_LANGUAGES = ['de', 'en'] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

/**
 * Best-effort system language: the first of the browser/OS preferred
 * languages (the Tauri webview inherits them from the OS) that Aperio
 * ships, else English. Used as the default when the user hasn't chosen an
 * explicit language (`locale` pref unset or `"system"`).
 */
export function detectSystemLanguage(): SupportedLanguage {
  const prefs =
    typeof navigator !== 'undefined'
      ? navigator.languages && navigator.languages.length > 0
        ? navigator.languages
        : [navigator.language]
      : [];
  for (const tag of prefs) {
    const base = tag?.toLowerCase().split(/[-_]/, 1)[0];
    if (base === 'de') return 'de';
    if (base === 'en') return 'en';
  }
  return 'en';
}

void i18n.use(initReactI18next).init({
  resources: {
    de: { translation: de },
    en: { translation: en },
  },
  // Default to the system language. An explicit, synced choice (the
  // `locale` user-pref) is applied on app start by `applyStoredLanguage`.
  lng: detectSystemLanguage(),
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
});

export default i18n;
