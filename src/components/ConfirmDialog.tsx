import { useEffect, useId, useRef } from 'react';
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
  /**
   * Optional middle actions rendered between Cancel and the primary button —
   * for a 3-way choice (e.g. "cancel the meeting and notify attendees" /
   * "remove without notifying" / "keep"). Each closes the dialog after running.
   */
  extraActions?: Array<{ label: string; onClick: () => void; danger?: boolean }>;
}

export function ConfirmDialog({
  isOpen,
  onClose,
  onConfirm,
  title,
  message,
  confirmLabel,
  cancelLabel,
  describedById,
  extraActions,
}: ConfirmDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const messageId = useId();

  // The message names WHICH item is about to be deleted. It lives in Modal's
  // `role="application"` body, where a static <p> is invisible to focus-mode
  // traversal — the user would confirm a destructive action without ever
  // hearing the target. Describe every action button with it (focus opens on
  // Cancel, so it is spoken the instant the dialog opens, and re-spoken as the
  // user Tabs to the danger/extra buttons). Merge any caller-supplied hint.
  const describedBy =
    [messageId, describedById].filter(Boolean).join(' ') || undefined;

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
      <p id={messageId} className="form__message">
        {message}
      </p>
      <div className="form__actions">
        <button
          ref={cancelRef}
          type="button"
          onClick={onClose}
          className="form__action"
          aria-describedby={describedBy}
        >
          {cancelLabel ?? t('dialogs.confirm.cancel')}
        </button>
        {extraActions?.map((action) => (
          <button
            key={action.label}
            type="button"
            onClick={() => {
              action.onClick();
              onClose();
            }}
            className={`form__action${
              action.danger ? ' form__action--danger' : ''
            }`}
            aria-describedby={describedBy}
          >
            {action.label}
          </button>
        ))}
        <button
          type="button"
          onClick={() => {
            onConfirm();
            onClose();
          }}
          className="form__action form__action--danger"
          aria-describedby={describedBy}
        >
          {confirmLabel ?? t('dialogs.confirm.confirm')}
        </button>
      </div>
    </Modal>
  );
}
