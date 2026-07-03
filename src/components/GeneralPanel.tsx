import { addDays, format } from 'date-fns';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { getUserPref, setUserPref, trayAvailable } from '../api/client';
import { localeFor } from '../intl/dateFormat';
import {
  readLanguagePref,
  setLanguagePref,
  type LanguagePref,
} from '../intl/language';
import { useThemeMode, type ThemeMode } from '../state/themeMode';
import {
  UI_MAX_SCALE,
  UI_MIN_SCALE,
  UI_SCALE_STEP,
  useUiScale,
} from '../state/uiScale';
import { type WeekStart } from '../state/viewMath';
import { useViewState } from '../state/viewStateContext';

const CLOSE_TO_TRAY = 'window.closeToTray';
const MINIMIZE_TO_TRAY = 'window.minimizeToTray';

/**
 * General app settings (Settings → Allgemein):
 *
 *  - **Language.** System / Deutsch / English. Persists to the synced
 *    `locale` pref and applies live via i18next.
 *  - **System tray.** Whether closing / minimizing tucks Aperio into the
 *    tray (where the reminder scheduler keeps running) instead of quitting /
 *    going to the taskbar. Both default off and persist to synced prefs;
 *    gated on `tray_available` so they disable where there's no tray.
 */
export function GeneralPanel() {
  const { t, i18n } = useTranslation();
  const { weekStartsOn, setWeekStartsOn } = useViewState();
  const [uiScale, setUiScale] = useUiScale();
  const [themeMode, setThemeMode] = useThemeMode();
  const [language, setLanguage] = useState<LanguagePref>('system');

  // Current scale as a percent label — the slider's spoken value
  // (aria-valuetext) and the visible read-out next to it.
  const fontSizeLabel = t('dialogs.settings.general.fontSizeOption', {
    pct: Math.round(uiScale * 100),
  });

  // Localized weekday names for the week-start picker. 7 Jan 2024 is a
  // Sunday (date-fns weekStartsOn 0), so option index d maps to that weekday.
  const weekdayOptions = useMemo(() => {
    const locale = localeFor(i18n.language);
    const sundayRef = new Date(2024, 0, 7);
    return Array.from({ length: 7 }, (_, d) => ({
      value: d as WeekStart,
      label: format(addDays(sundayRef, d), 'EEEE', { locale }),
    }));
  }, [i18n.language]);
  // `null` = still probing tray availability.
  const [available, setAvailable] = useState<boolean | null>(null);
  const [closeToTray, setCloseToTray] = useState(false);
  const [minimizeToTray, setMinimizeToTray] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const langPref = await readLanguagePref();
      if (!cancelled) setLanguage(langPref);
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
        // tray as unavailable so those controls stay disabled.
        // eslint-disable-next-line no-console
        console.warn('tray availability probe failed', err);
        setAvailable(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const onLanguageChange = (value: LanguagePref) => {
    setLanguage(value);
    void setLanguagePref(value);
  };

  const persist = (
    key: string,
    value: boolean,
    setter: (v: boolean) => void,
  ) => {
    setter(value);
    void setUserPref(key, value ? 'true' : 'false');
  };

  const trayDisabled = available !== true;

  return (
    <div className="settings-panel general-panel">
      <FocusableNote className="form__hint">
        {t('dialogs.settings.general.hint')}
      </FocusableNote>

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.general.languageHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.general.languageHeading')}
        </h3>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.general.languageLabel')}
          </span>
          <select
            value={language}
            onChange={(e) => onLanguageChange(e.target.value as LanguagePref)}
          >
            <option value="system">
              {t('dialogs.settings.general.languageSystem')}
            </option>
            <option value="de">
              {t('dialogs.settings.general.languageGerman')}
            </option>
            <option value="en">
              {t('dialogs.settings.general.languageEnglish')}
            </option>
          </select>
        </label>
      </section>

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.general.viewsHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.general.viewsHeading')}
        </h3>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.general.weekStartLabel')}
          </span>
          <select
            value={weekStartsOn}
            onChange={(e) =>
              setWeekStartsOn(Number(e.target.value) as WeekStart)
            }
          >
            {weekdayOptions.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <p className="form__hint">
          {t('dialogs.settings.general.weekStartHint')}
        </p>
      </section>

      <section
        className="general-panel__section"
        aria-label={t('dialogs.settings.general.appearanceHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.general.appearanceHeading')}
        </h3>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.general.fontSizeLabel')}
          </span>
          <span className="general-panel__scale-row">
            <input
              type="range"
              className="general-panel__scale-slider"
              min={UI_MIN_SCALE}
              max={UI_MAX_SCALE}
              step={UI_SCALE_STEP}
              value={uiScale}
              // Announce "120 %" instead of the raw 1.2; updates as the user
              // arrows the slider (±5%).
              aria-valuetext={fontSizeLabel}
              onChange={(e) => setUiScale(Number.parseFloat(e.target.value))}
            />
            {/* Visible read-out for sighted users; the slider already speaks
                the value via aria-valuetext, so hide this from the SR. */}
            <span className="general-panel__scale-value" aria-hidden="true">
              {fontSizeLabel}
            </span>
          </span>
        </label>
        <p className="form__hint">
          {t('dialogs.settings.general.fontSizeHint')}
        </p>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.general.themeLabel')}
          </span>
          <select
            value={themeMode}
            onChange={(e) => setThemeMode(e.target.value as ThemeMode)}
          >
            <option value="system">
              {t('dialogs.settings.general.themeSystem')}
            </option>
            <option value="light">
              {t('dialogs.settings.general.themeLight')}
            </option>
            <option value="dark">
              {t('dialogs.settings.general.themeDark')}
            </option>
          </select>
        </label>
        <p className="form__hint">
          {t('dialogs.settings.general.themeHint')}
        </p>
      </section>

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
            disabled={trayDisabled}
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
            disabled={trayDisabled}
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
