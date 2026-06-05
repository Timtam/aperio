import { getUserPref, setUserPref } from '../api/client';
import i18n, { detectSystemLanguage, type SupportedLanguage } from '../i18n';

/**
 * The persisted language choice (synced via the `locale` user-pref).
 * `"system"` follows the OS language; otherwise a concrete language wins.
 */
export type LanguagePref = 'system' | SupportedLanguage;

const LOCALE_PREF = 'locale';

/** Resolve a stored choice to the concrete language i18next should use. */
export function resolveLanguage(pref: LanguagePref): SupportedLanguage {
  return pref === 'system' ? detectSystemLanguage() : pref;
}

/** Normalise the opaque stored string into a {@link LanguagePref}. */
export function parseLanguagePref(stored: string | null): LanguagePref {
  return stored === 'de' || stored === 'en' ? stored : 'system';
}

/** Read the persisted choice; defaults to `"system"`. */
export async function readLanguagePref(): Promise<LanguagePref> {
  try {
    return parseLanguagePref(await getUserPref(LOCALE_PREF));
  } catch {
    return 'system';
  }
}

/**
 * Apply the persisted/synced choice to i18next. Called once on app start
 * (the synchronous init already defaults to the system language, so this
 * only changes anything when the user picked an explicit language — locally
 * or on another device).
 */
export async function applyStoredLanguage(): Promise<void> {
  const lang = resolveLanguage(await readLanguagePref());
  if (i18n.language !== lang) {
    await i18n.changeLanguage(lang);
  }
}

/** Persist + apply a new choice (from the General settings selector). */
export async function setLanguagePref(pref: LanguagePref): Promise<void> {
  await setUserPref(LOCALE_PREF, pref);
  const lang = resolveLanguage(pref);
  if (i18n.language !== lang) {
    await i18n.changeLanguage(lang);
  }
}
