import { useEffect } from 'react';

import { getUserPref, setUserPref } from '../api/prefs';
import i18n, {
  detectSystemLanguage,
  SUPPORTED_LANGUAGES,
  type SupportedLanguage,
} from '../../i18n';

// Manual app-language override, SYNCED across the user's devices via the
// `locale` user-pref (a §19.2.1 always-sync key) — the mobile twin of the
// desktop src/intl/language.ts. i18n initialises synchronously from the device
// locale (i18n.ts `detectSystemLanguage`); this layer applies the stored choice
// over that default on launch, and writing it propagates to other devices on
// the next sync round. (Previously device-local in AsyncStorage; moved onto the
// synced pref for desktop parity — a language picked on one device follows.)

/** `'system'` follows the device locale; otherwise a pinned shipped language. */
export type LanguageChoice = 'system' | SupportedLanguage;

const LOCALE_PREF = 'locale';

/** Normalise the opaque stored pref string into a {@link LanguageChoice}. */
function parseLanguageChoice(stored: string | null): LanguageChoice {
  if (stored != null && (SUPPORTED_LANGUAGES as readonly string[]).includes(stored)) {
    return stored as SupportedLanguage;
  }
  return 'system';
}

/** The stored choice (from the synced `locale` pref), or `'system'` when
 *  unset/unreadable (e.g. the store isn't open yet — falls back to the
 *  device-locale default already applied at init). */
export async function readLanguageChoice(): Promise<LanguageChoice> {
  try {
    return parseLanguageChoice(await getUserPref(LOCALE_PREF));
  } catch {
    return 'system';
  }
}

/** Persist the choice to the synced `locale` pref (propagates to other devices
 *  on the next round). Best-effort — the override still applies this session. */
export async function writeLanguageChoice(choice: LanguageChoice): Promise<void> {
  try {
    await setUserPref(LOCALE_PREF, choice);
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

/** Mount once near the app root: apply the stored/synced choice on launch
 *  (after the synchronous device-locale init), so a pinned language — set here
 *  or on another device — takes effect. */
export function useStoredLanguage(): void {
  useEffect(() => {
    void readLanguageChoice().then(applyLanguageChoice);
  }, []);
}
