import { addDays, format } from 'date-fns';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import {
  autostartIsEnabled,
  autostartSet,
  getUserPref,
  setUserPref,
  trayAvailable,
} from '../api/client';
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
import {
  TIME_STEP_CHOICES,
  type TimeStepMinutes,
  type WeekStart,
} from '../state/viewMath';
import { useViewState } from '../state/viewStateContext';

const CLOSE_TO_TRAY = 'window.closeToTray';
const MINIMIZE_TO_TRAY = 'window.minimizeToTray';
// §17.4: whether an autostart launch starts hidden in the tray. Device-local;
// defaults ON (unset ⇒ on) — matches the backend `pref_bool(..., true)`.
const AUTOSTART_MINIMIZED = 'window.autostartMinimized';

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
  const {
    weekStartsOn,
    setWeekStartsOn,
    timeStepMinutes,
    setTimeStepMinutes,
    startOnToday,
    setStartOnToday,
    showCancelledEvents,
    setShowCancelledEvents,
    showHiddenCalendarTargets,
    setShowHiddenCalendarTargets,
  } = useViewState();
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
  // Autostart (§17.4). `null` = still probing / unsupported (e.g. outside the
  // Tauri runtime); the OS registration is the source of truth for the value.
  const [autostart, setAutostart] = useState(false);
  const [autostartSupported, setAutostartSupported] = useState<boolean | null>(
    null,
  );
  // Whether an autostart launch starts hidden in the tray (device-local pref,
  // default on). Only meaningful when a tray exists.
  const [autostartMinimized, setAutostartMinimized] = useState(true);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const langPref = await readLanguagePref();
      if (!cancelled) setLanguage(langPref);
      try {
        const [avail, closePref, minPref, autoMinPref] = await Promise.all([
          trayAvailable(),
          getUserPref(CLOSE_TO_TRAY),
          getUserPref(MINIMIZE_TO_TRAY),
          getUserPref(AUTOSTART_MINIMIZED),
        ]);
        if (cancelled) return;
        // Default ON: only an explicit "false" turns minimized-start off.
        setAutostartMinimized(autoMinPref !== 'false');
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
      // Probe autostart independently so a tray-probe failure doesn't hide it
      // (and vice versa).
      try {
        const enabled = await autostartIsEnabled();
        if (cancelled) return;
        setAutostart(enabled);
        setAutostartSupported(true);
      } catch (err) {
        if (cancelled) return;
        // eslint-disable-next-line no-console
        console.warn('autostart probe failed', err);
        setAutostartSupported(false);
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

  // Autostart has no pref — the OS registration is the source of truth. Write it
  // optimistically and revert the toggle if the OS call fails.
  const onToggleAutostart = (next: boolean) => {
    setAutostart(next);
    void autostartSet(next).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn('autostart set failed', err);
      setAutostart(!next);
    });
  };

  const trayDisabled = available !== true;
  const autostartDisabled = autostartSupported !== true;

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
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.general.timeStepLabel')}
          </span>
          <select
            value={timeStepMinutes}
            onChange={(e) =>
              setTimeStepMinutes(Number(e.target.value) as TimeStepMinutes)
            }
          >
            {TIME_STEP_CHOICES.map((m) => (
              <option key={m} value={m}>
                {t('dialogs.settings.general.timeStepOption', { count: m })}
              </option>
            ))}
          </select>
        </label>
        <p className="form__hint">
          {t('dialogs.settings.general.timeStepHint')}
        </p>
        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={startOnToday}
            onChange={(e) => setStartOnToday(e.target.checked)}
          />
          <span>{t('dialogs.settings.general.startOnTodayLabel')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.general.startOnTodayHint')}
        </p>
        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={showCancelledEvents}
            onChange={(e) => setShowCancelledEvents(e.target.checked)}
          />
          <span>{t('dialogs.settings.general.showCancelledLabel')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.general.showCancelledHint')}
        </p>
        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={showHiddenCalendarTargets}
            onChange={(e) => setShowHiddenCalendarTargets(e.target.checked)}
          />
          <span>{t('dialogs.settings.general.showHiddenCalendarTargetsLabel')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.general.showHiddenCalendarTargetsHint')}
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
        aria-label={t('dialogs.settings.general.startupHeading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.general.startupHeading')}
        </h3>
        {autostartSupported === false && (
          <FocusableNote className="form__hint">
            {t('dialogs.settings.general.autostartUnavailable')}
          </FocusableNote>
        )}
        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={autostart}
            disabled={autostartDisabled}
            onChange={(e) => onToggleAutostart(e.target.checked)}
          />
          <span>{t('dialogs.settings.general.autostartLabel')}</span>
        </label>
        <p className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.general.autostartHint')}
        </p>
        {autostart && (
          <>
            <label className="general-panel__toggle">
              <input
                type="checkbox"
                checked={autostartMinimized}
                disabled={trayDisabled}
                onChange={(e) =>
                  persist(
                    AUTOSTART_MINIMIZED,
                    e.target.checked,
                    setAutostartMinimized,
                  )
                }
              />
              <span>
                {t('dialogs.settings.general.autostartMinimizedLabel')}
              </span>
            </label>
            <p className="form__hint general-panel__toggle-hint">
              {t('dialogs.settings.general.autostartMinimizedHint')}
            </p>
          </>
        )}
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
