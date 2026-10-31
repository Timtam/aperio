import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { useNotificationShortcut } from '../state/useNotificationShortcut';
import { Modal } from './Modal';

interface NotificationShortcutHintDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

/**
 * The one-time hint: a reminder came due while Windows would hide it,
 * because Aperio has no Start menu shortcut for notifications. Offers to
 * set it up now; Settings, General has the same at any time.
 *
 * Focus opens on the explanation, so NVDA reads why before the buttons.
 * While setting up runs, Escape and "Not now" do nothing: the success path
 * closes this dialog, and closing it first would let that close whatever
 * dialog the user opened next. A failure keeps it open with the alert.
 */
export function NotificationShortcutHintDialog({
  isOpen,
  onClose,
}: NotificationShortcutHintDialogProps) {
  const { t } = useTranslation();
  const bodyRef = useRef<HTMLParagraphElement>(null);
  const { busy, error, setUp } = useNotificationShortcut({ onSetUp: onClose });
  const working = busy != null;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.notificationShortcut.hintTitle')}
      className="modal--confirm"
      dismissOnBackdrop={false}
      initialFocusRef={bodyRef}
      busy={working}
    >
      <div className="form">
        <FocusableNote ref={bodyRef}>{t('dialogs.notificationShortcut.hintBody')}</FocusableNote>
        {error != null && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}
        <div className="form__actions">
          <button
            type="button"
            className="form__action form__action--primary"
            aria-disabled={working}
            aria-busy={working}
            onClick={setUp}
          >
            {t('dialogs.notificationShortcut.setUp')}
          </button>
          <button
            type="button"
            className="form__action"
            aria-disabled={working}
            onClick={() => {
              if (!working) onClose();
            }}
          >
            {t('dialogs.notificationShortcut.hintLater')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
