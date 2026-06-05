import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { requestWindowClose, requestWindowMinimize } from '../api/client';

/**
 * Custom title bar.
 *
 * The system decorations are disabled (`decorations: false` in
 * `tauri.conf.json`), so the app has to draw its own title bar. This
 * component implements:
 *
 *  - A drag region (the entire bar minus the buttons) marked with
 *    `data-tauri-drag-region`, so dragging the bar moves the window.
 *  - Min / toggle-max / close buttons. They live on the *right* on
 *    Windows/Linux and on the *left* on macOS, matching native
 *    expectations.
 *  - `aria-label`s on every button so screen readers announce them.
 *  - A live indicator on the max button so its label switches between
 *    "Maximize" and "Restore" as the window state changes.
 *
 * The buttons are real `<button>` elements — keyboard focus, Enter/Space
 * activation, and screen-reader semantics come for free.
 */
export function TitleBar() {
  const { t } = useTranslation();
  const [isMaximized, setIsMaximized] = useState(false);

  // Platform sniff via UA: Tauri's WebView leaks the host platform in the
  // user agent, which is good enough for "should the buttons live on the
  // left like macOS or on the right like the rest of the world".
  const isMac =
    typeof navigator !== 'undefined' &&
    /Mac|iPhone|iPad/i.test(navigator.userAgent);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;

    win
      .isMaximized()
      .then(setIsMaximized)
      .catch(() => {});

    win
      .onResized(() => {
        win
          .isMaximized()
          .then(setIsMaximized)
          .catch(() => {});
      })
      .then((un) => {
        unlisten = un;
      })
      .catch(() => {});

    return () => unlisten?.();
  }, []);

  const win = () => getCurrentWindow();
  // Minimize / close route through the backend so the "… to tray" prefs can
  // hide the window instead, when the user enabled them and a tray exists.
  // Maximize stays a plain window op.
  const onMinimize = () => void requestWindowMinimize();
  const onToggleMaximize = () => void win().toggleMaximize();
  const onClose = () => void requestWindowClose();

  const buttons = (
    <div
      className="title-bar__buttons"
      role="group"
      aria-label={t('app.titleBar.windowControls')}
    >
      <button
        type="button"
        className="title-bar__btn title-bar__btn--min"
        onClick={onMinimize}
        aria-label={t('app.titleBar.minimize')}
      >
        <span aria-hidden="true">—</span>
      </button>
      <button
        type="button"
        className="title-bar__btn title-bar__btn--max"
        onClick={onToggleMaximize}
        aria-label={
          isMaximized ? t('app.titleBar.restore') : t('app.titleBar.maximize')
        }
        aria-pressed={isMaximized}
      >
        <span aria-hidden="true">{isMaximized ? '❐' : '☐'}</span>
      </button>
      <button
        type="button"
        className="title-bar__btn title-bar__btn--close"
        onClick={onClose}
        aria-label={t('app.titleBar.close')}
      >
        <span aria-hidden="true">✕</span>
      </button>
    </div>
  );

  return (
    <div
      className={`title-bar${isMac ? ' title-bar--mac' : ''}`}
      data-tauri-drag-region
    >
      {isMac && buttons}
      <div className="title-bar__title" data-tauri-drag-region>
        {t('app.title')}
      </div>
      {!isMac && buttons}
    </div>
  );
}
