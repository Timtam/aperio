import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { setTrayLabels } from '../api/client';

/**
 * Keep the system-tray menu labels in sync with the app language.
 *
 * The tray menu is built host-side at startup with placeholder labels (the
 * host has no i18n). This pushes the localized labels once i18n is ready and
 * again whenever the language changes. No-op on platforms without a tray
 * (the command is a no-op there) and outside the Tauri runtime (the invoke
 * rejects, which we swallow).
 */
export function useTrayMenuLabels(): void {
  const { t, i18n } = useTranslation();
  useEffect(() => {
    void setTrayLabels(t('tray.show'), t('tray.quit')).catch(() => {
      // No tray, or not running under Tauri — nothing to label.
    });
  }, [t, i18n.language]);
}
