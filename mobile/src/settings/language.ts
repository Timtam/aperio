import AsyncStorage from '@react-native-async-storage/async-storage';
import { useEffect } from 'react';

import i18n, {
  detectSystemLanguage,
  SUPPORTED_LANGUAGES,
  type SupportedLanguage,
} from '../../i18n';

// Manual app-language override. i18n initialises synchronously from the device
// locale (i18n.ts `detectSystemLanguage`); this layer lets the user pin a
// language instead — read from AsyncStorage on launch and applied over the
// detected default. Mirrors the desktop Settings → language control.

/** `'system'` follows the device locale; otherwise a pinned shipped language. */
export type LanguageChoice = 'system' | SupportedLanguage;

const STORAGE_KEY = 'aperio.settings.language';

/** The stored choice, or `'system'` when unset/unreadable. */
export async function readLanguageChoice(): Promise<LanguageChoice> {
  try {
    const raw = await AsyncStorage.getItem(STORAGE_KEY);
    if (raw === 'system') return 'system';
    if (raw != null && (SUPPORTED_LANGUAGES as readonly string[]).includes(raw)) {
      return raw as SupportedLanguage;
    }
  } catch {
    // Storage unavailable — fall through to the system default.
  }
  return 'system';
}

/** Persist the choice. Best-effort (the override is a convenience). */
export async function writeLanguageChoice(choice: LanguageChoice): Promise<void> {
  try {
    await AsyncStorage.setItem(STORAGE_KEY, choice);
  } catch {
    // Ignore — the app still runs in the chosen language for this session.
  }
}

/** Resolve a choice to a concrete shipped language (`'system'` → detected). */
export function resolveLanguage(choice: LanguageChoice): SupportedLanguage {
  return choice === 'system' ? detectSystemLanguage() : choice;
}

/** Switch i18n to `choice` now (re-renders every `useTranslation` consumer). */
export async function applyLanguageChoice(choice: LanguageChoice): Promise<void> {
  const lang = resolveLanguage(choice);
  if (i18n.language !== lang) await i18n.changeLanguage(lang);
}

/** Mount once near the app root: apply the stored override on launch (after the
 *  synchronous device-locale init), so a pinned language survives a restart. */
export function useStoredLanguage(): void {
  useEffect(() => {
    void readLanguageChoice().then(applyLanguageChoice);
  }, []);
}
