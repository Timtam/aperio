import { useId } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import {
  NOTIFICATION_SHORTCUT_STATUS_KEY as STATUS_KEY,
  useNotificationShortcut,
} from '../state/useNotificationShortcut';

/**
 * Windows notifications: why the Start menu shortcut is needed, where it
 * stands, and the buttons to set it up or remove it. Nothing renders off
 * Windows. `settings` is the Settings > General section with its own
 * heading; `wizard` is the first-launch wizard's step body (the wizard owns
 * the heading) and offers only setting up.
 *
 * Two fixed buttons rather than one whose label flips: a second Enter
 * right after setting up must not remove the entry again. The one that
 * does not apply is aria-disabled, never natively disabled, so focus never
 * drops off it.
 */
export function NotificationShortcutSection({ variant }: { variant: 'settings' | 'wizard' }) {
  const { t } = useTranslation();
  const { phase, busy, error, setUp, remove } = useNotificationShortcut();
  const removeHintId = useId();

  if (phase.kind === 'loading') return null;
  if (phase.kind === 'ready' && phase.status.kind === 'unsupported') return null;

  const status = phase.kind === 'ready' ? phase.status : null;
  let statusText: string;
  if (status == null) {
    statusText = t('dialogs.notificationShortcut.statusUnknown', {
      error: phase.kind === 'probeFailed' ? phase.error : '',
    });
  } else if (status.kind === 'points_elsewhere') {
    statusText = t(STATUS_KEY.points_elsewhere, { target: status.target });
  } else if (status.kind === 'unsupported') {
    return null;
  } else {
    statusText = t(STATUS_KEY[status.kind]);
  }
  const actionable = status != null && status.kind !== 'dev_build';
  const canSetUp = status != null && (status.needs_set_up || status.kind === 'points_elsewhere');
  const canRemove =
    status != null && (status.kind === 'set_up' || status.kind === 'points_elsewhere');
  const showRemove = variant === 'settings';

  const body = (
    <>
      <FocusableNote className="form__hint">
        {t('dialogs.notificationShortcut.intro')}
      </FocusableNote>
      <FocusableNote className="form__hint">{statusText}</FocusableNote>
      {error != null && (
        <p role="alert" className="form__error">
          {error}
        </p>
      )}
      {actionable && (
        <div className="form__actions">
          <button
            type="button"
            aria-disabled={!canSetUp || busy != null}
            aria-busy={busy === 'setUp'}
            onClick={() => {
              if (canSetUp) setUp();
            }}
          >
            {t('dialogs.notificationShortcut.setUp')}
          </button>
          {showRemove && (
            <button
              type="button"
              aria-disabled={!canRemove || busy != null}
              aria-busy={busy === 'remove'}
              aria-describedby={removeHintId}
              onClick={() => {
                if (canRemove) remove();
              }}
            >
              {t('dialogs.notificationShortcut.remove')}
            </button>
          )}
        </div>
      )}
      {actionable && showRemove && (
        <p id={removeHintId} className="form__hint">
          {t('dialogs.notificationShortcut.removeHint')}
        </p>
      )}
    </>
  );

  if (variant === 'wizard') return body;
  return (
    <section
      className="general-panel__section"
      aria-label={t('dialogs.notificationShortcut.heading')}
    >
      <h3 className="calendars-panel__account">{t('dialogs.notificationShortcut.heading')}</h3>
      {body}
    </section>
  );
}
