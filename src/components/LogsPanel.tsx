import { save } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  clearLogs,
  collectLogs,
  exportLogs,
  getLogLevel,
  getRecentLogs,
  isCommandError,
  logsDirPath,
  setLogLevel,
  type LogLevel,
} from '../api/client';

const LEVELS: LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace'];

/** Human-readable message from a thrown value (CommandError → its message). */
function errMessage(err: unknown): string {
  return isCommandError(err) ? err.message : String(err);
}

/**
 * Settings → Protokolle (diagnostics).
 *
 * Lets a user pick the log verbosity, peek at the most recent lines, and
 * export the full log bundle (redacted by default) to send for support — the
 * point being that release builds otherwise have no visible log. Backed by the
 * file-logging layer in `logging.rs`; the level is a device-local pref.
 */
export function LogsPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();

  const [level, setLevel] = useState<LogLevel>('info');
  const [redact, setRedact] = useState(true);
  const [recent, setRecent] = useState('');
  const [dirPath, setDirPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const text = await getRecentLogs(500);
      setRecent(text);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('get_recent_logs failed', err);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [lvl, path] = await Promise.all([getLogLevel(), logsDirPath()]);
        if (cancelled) return;
        if (LEVELS.includes(lvl as LogLevel)) setLevel(lvl as LogLevel);
        setDirPath(path);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('logs panel init failed', err);
      }
      await refresh();
    })();
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  const onLevelChange = (next: LogLevel) => {
    const prev = level;
    setLevel(next);
    void setLogLevel(next).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn('set_log_level failed', err);
      // Roll the UI back so it doesn't claim a level the backend rejected.
      setLevel(prev);
      setError(errMessage(err));
    });
  };

  const onExport = useCallback(async () => {
    setError(null);
    setBusy(true);
    try {
      const path = await save({
        defaultPath: 'aperio-logs.txt',
        filters: [{ name: 'Log', extensions: ['txt', 'log'] }],
      });
      if (!path) return;
      await exportLogs(path, redact);
      announce(t('dialogs.settings.logs.exported'));
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }, [redact, announce, t]);

  const onCopy = useCallback(async () => {
    setError(null);
    try {
      setBusy(true);
      const text = await collectLogs(redact);
      await navigator.clipboard.writeText(text);
      announce(t('dialogs.settings.logs.copied'));
    } catch (err) {
      setError(errMessage(err));
    } finally {
      setBusy(false);
    }
  }, [redact, announce, t]);

  const onCopyPath = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(dirPath);
      announce(t('dialogs.settings.logs.pathCopied'));
    } catch {
      /* clipboard unavailable — ignore */
    }
  }, [dirPath, announce, t]);

  const onClear = useCallback(async () => {
    if (!window.confirm(t('dialogs.settings.logs.clearConfirm'))) return;
    setError(null);
    try {
      await clearLogs();
      await refresh();
      announce(t('dialogs.settings.logs.cleared'));
    } catch (err) {
      setError(errMessage(err));
    }
  }, [refresh, announce, t]);

  return (
    <div className="settings-panel logs-panel">
      <FocusableNote className="form__hint">
        {t('dialogs.settings.logs.hint')}
      </FocusableNote>

      {error && (
        <p role="alert" className="form__error">
          {error}
        </p>
      )}

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.logs.levelHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.logs.levelHeading')}
        </h3>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.logs.levelLabel')}
          </span>
          <select
            value={level}
            onChange={(e) => onLevelChange(e.target.value as LogLevel)}
          >
            {LEVELS.map((lv) => (
              <option key={lv} value={lv}>
                {t(`dialogs.settings.logs.level.${lv}`)}
              </option>
            ))}
          </select>
        </label>
        <p className="form__hint">{t('dialogs.settings.logs.levelHint')}</p>
      </section>

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.logs.exportHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.logs.exportHeading')}
        </h3>
        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={redact}
            onChange={(e) => setRedact(e.target.checked)}
          />
          <span>{t('dialogs.settings.logs.redact')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.logs.redactHint')}
        </p>
        <div className="form__actions">
          <button
            type="button"
            className="form__action"
            onClick={() => void onExport()}
            aria-disabled={busy || undefined}
          >
            {t('dialogs.settings.logs.export')}
          </button>
          <button
            type="button"
            className="form__action"
            onClick={() => void onCopy()}
            aria-disabled={busy || undefined}
          >
            {t('dialogs.settings.logs.copy')}
          </button>
          <button
            type="button"
            className="form__action form__action--secondary"
            onClick={() => void onClear()}
          >
            {t('dialogs.settings.logs.clear')}
          </button>
        </div>
        {dirPath && (
          <p className="form__hint">
            {t('dialogs.settings.logs.location', { path: dirPath })}{' '}
            <button
              type="button"
              className="link-button"
              onClick={() => void onCopyPath()}
            >
              {t('dialogs.settings.logs.copyPath')}
            </button>
          </p>
        )}
      </section>

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.logs.viewHeading')}
      >
        <div className="logs-panel__view-head">
          <h3 className="calendars-panel__account">
            {t('dialogs.settings.logs.viewHeading')}
          </h3>
          <button
            type="button"
            className="form__action"
            onClick={() => void refresh()}
          >
            {t('dialogs.settings.logs.refresh')}
          </button>
        </div>
        <pre className="logs-panel__output" aria-live="off" tabIndex={0}>
          {recent || t('dialogs.settings.logs.empty')}
        </pre>
      </section>
    </div>
  );
}
