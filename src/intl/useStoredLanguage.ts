import { useEffect } from 'react';

import { applyStoredLanguage } from './language';

/**
 * Apply the persisted/synced language choice once on app start. The
 * synchronous i18next init already defaults to the system language, so this
 * only switches when the user picked an explicit language (here or on
 * another synced device).
 */
export function useStoredLanguage(): void {
  useEffect(() => {
    void applyStoredLanguage();
  }, []);
}
