import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * Generic confirmation dialog used for destructive actions (delete an
 * event / task, discard unsaved changes, ...).
 *
 * The dialog renders through {@link Modal} so it inherits the focus
 * trap, inert-shell and focus restore on close — i.e. the same a11y
 * guarantees the spec demands for every dialog. The initial focus
 * lands on the *Cancel* button so accidentally hammering Enter does
 * not delete the row. Users have to deliberately Tab to the danger
 * button (or use the mouse) before confirming.
 */
export interface ConfirmDialogProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: string;
  /** Body text shown above the action row. */
  message: string;
  /** Optional label for the danger button (defaults to t('dialogs.confirm.confirm')). */
  confirmLabel?: string;
  /** Optional label for the cancel button. */
  cancelLabel?: string;
  /** Hint passed through to assistive tech via aria-describedby. */
  describedById?: string;
}

export function ConfirmDialog({
  isOpen,
  onClose,
  onConfirm,
  title,
  message,
  confirmLabel,
  cancelLabel,
}: ConfirmDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);

  // Focus the cancel button first — see component docstring for why.
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => cancelRef.current?.focus());
  }, [isOpen]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      className="modal--confirm"
      dismissOnBackdrop={false}
    >
      <p className="form__message">{message}</p>
      <div className="form__actions">
        <button
          ref={cancelRef}
          type="button"
          onClick={onClose}
          className="form__action"
        >
          {cancelLabel ?? t('dialogs.confirm.cancel')}
        </button>
        <button
          type="button"
          onClick={() => {
            onConfirm();
            onClose();
          }}
          className="form__action form__action--danger"
        >
          {confirmLabel ?? t('dialogs.confirm.confirm')}
        </button>
      </div>
    </Modal>
  );
}
