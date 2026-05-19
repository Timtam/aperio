import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

type AppInfo = {
  name: string;
  version: string;
};

/**
 * Phase 0 app shell.
 *
 * Architectural markers that must survive future refactors:
 *  - `role="application"` on the root element (DESIGN.md section 3.2.1)
 *    so screen readers stay in focus mode.
 *  - `aria-label` carrying the product name.
 *  - A global `aria-live` region (added in Phase 2).
 *
 * In this phase the app is just a liveness check: it calls the
 * `app_info` Tauri command and renders the name + version.
 */
export function App() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppInfo>('app_info')
      .then(setInfo)
      .catch((err) => setError(String(err)));
  }, []);

  return (
    <div id="app-root" role="application" aria-label="Aperio" className="app-root">
      <header className="app-header">
        <h1>{t('app.title')}</h1>
      </header>
      <main className="app-main">
        <p>{t('app.phase0.intro')}</p>
        {info && (
          <p>
            <strong>{info.name}</strong> v{info.version}
          </p>
        )}
        {error && (
          <p role="alert" className="error">
            {t('app.phase0.invokeError', { error })}
          </p>
        )}
      </main>
    </div>
  );
}
