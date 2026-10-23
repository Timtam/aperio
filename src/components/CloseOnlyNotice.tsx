import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { Modal } from './Modal';

/**
 * What a dialog shows in place of its form once a write landed in part: the
 * sentence, focused, and Close. A new series that may stand (decisions 203,
 * 210), an item created without one of its follow-up steps (273), or a move
 * whose original stayed behind (277) — saving again would write twice what
 * already stands, so the form never comes back.
 */
export function CloseOnlyNotice({
  title,
  sentence,
  onClose,
  className = 'modal--form',
}: {
  title: string;
  sentence: string;
  onClose: () => void;
  className?: string;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    ref.current?.focus();
  }, [sentence]);
  return (
    <Modal
      isOpen
      onClose={onClose}
      title={title}
      className={className}
      dismissOnBackdrop={false}
    >
      <div className="form">
        <FocusableNote ref={ref} className="form__hint form__hint--warning">
          {sentence}
        </FocusableNote>
        <div className="form__actions">
          <button
            type="button"
            className="form__action form__action--primary"
            onClick={onClose}
          >
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
