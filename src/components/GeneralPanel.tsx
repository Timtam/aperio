import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { getUserPref, setUserPref, trayAvailable } from '../api/client';

const CLOSE_TO_TRAY = 'window.closeToTray';
const MINIMIZE_TO_TRAY = 'window.minimizeToTray';

/**
 * General app settings (Settings → Allgemein).
 *
 * Currently just the system-tray behaviour: whether closing and/or
 * minimizing the window tucks Aperio into the tray (where the reminder
 * scheduler keeps running) instead of quitting / going to the taskbar.
 *
 * Both toggles persist to `user_prefs` and are read by the backend's
 * window-event handlers. They're gated on `tray_available`: on a desktop
 * with no tray (e.g. GNOME without an AppIndicator extension) the controls
 * disable and a hint explains why — matching the host, which falls back to
 * normal close/minimize there regardless.
 */
export function GeneralPanel() {
  const { t } = useTranslation();
  // `null` = still probing tray availability.
  const [available, setAvailable] = useState<boolean | null>(null);
  const [closeToTray, setCloseToTray] = useState(false);
  const [minimizeToTray, setMinimizeToTray] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [avail, closePref, minPref] = await Promise.all([
          trayAvailable(),
          getUserPref(CLOSE_TO_TRAY),
          getUserPref(MINIMIZE_TO_TRAY),
        ]);
        if (cancelled) return;
        setAvailable(avail);
        setCloseToTray(closePref === 'true');
        setMinimizeToTray(minPref === 'true');
      } catch (err) {
        if (cancelled) return;
        // Outside the Tauri runtime (or the command failed) → treat the
        // tray as unavailable so the controls stay disabled.
        // eslint-disable-next-line no-console
        console.warn('tray availability probe failed', err);
        setAvailable(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const persist = (
    key: string,
    value: boolean,
    setter: (v: boolean) => void,
  ) => {
    setter(value);
    void setUserPref(key, value ? 'true' : 'false');
  };

  const disabled = available !== true;

  return (
    <div className="settings-panel general-panel">
      <FocusableNote className="form__hint">
        {t('dialogs.settings.general.hint')}
      </FocusableNote>

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.general.trayHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.general.trayHeading')}
        </h3>
        <p className="form__hint">
          {t('dialogs.settings.general.trayIntro')}
        </p>

        {available === false && (
          <FocusableNote className="form__hint">
            {t('dialogs.settings.general.trayUnavailable')}
          </FocusableNote>
        )}

        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={closeToTray}
            disabled={disabled}
            onChange={(e) =>
              persist(CLOSE_TO_TRAY, e.target.checked, setCloseToTray)
            }
          />
          <span>{t('dialogs.settings.general.closeToTray')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.general.closeToTrayHint')}
        </p>

        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={minimizeToTray}
            disabled={disabled}
            onChange={(e) =>
              persist(MINIMIZE_TO_TRAY, e.target.checked, setMinimizeToTray)
            }
          />
          <span>{t('dialogs.settings.general.minimizeToTray')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.general.minimizeToTrayHint')}
        </p>
      </section>
    </div>
  );
}
