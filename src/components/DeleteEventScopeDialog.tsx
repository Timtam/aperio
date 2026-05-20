import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * Three-way "delete one occurrence vs the whole series vs cancel"
 * dialog for the keyboard shortcut path in the calendar views.
 *
 * The editor dialog (`EventDialog`) does the same conceptual choice
 * with a radio scope selector inside the form — but the views fire
 * a delete straight from `Delete` / `Backspace`, so a quick modal
 * with three explicit buttons is the faster keyboard interaction
 * than tabbing through a radio group.
 *
 * The buttons are ordered so the destructive choices come last and
 * Cancel takes initial focus — mashing Enter never accidentally
 * deletes anything. Both delete buttons live in the danger style
 * because both are irreversible; the series button is the
 * stronger of the two and therefore the right-most one.
 */
export interface DeleteEventScopeDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Event title shown inside the prompt. */
  title: string;
  /** Drop just this single occurrence (EXDATE on the master row). */
  onOccurrence: () => void;
  /** Drop the master row, which removes every past + future occurrence. */
  onSeries: () => void;
}

export function DeleteEventScopeDialog({
  isOpen,
  onClose,
  title,
  onOccurrence,
  onSeries,
}: DeleteEventScopeDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);

  // Cancel-first focus: same safety net as the regular ConfirmDialog.
  // The user has to deliberately tab to a delete button to act.
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => cancelRef.current?.focus());
  }, [isOpen]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.deleteScope.title')}
      className="modal--confirm modal--confirm-wide"
      dismissOnBackdrop={false}
    >
      <p className="form__message">
        {t('dialogs.deleteScope.message', { title })}
      </p>
      <div className="form__actions">
        <button
          ref={cancelRef}
          type="button"
          onClick={onClose}
          className="form__action"
        >
          {t('dialogs.deleteScope.cancel')}
        </button>
        <button
          type="button"
          onClick={() => {
            onOccurrence();
            onClose();
          }}
          className="form__action form__action--danger"
        >
          {t('dialogs.deleteScope.occurrence')}
        </button>
        <button
          type="button"
          onClick={() => {
            onSeries();
            onClose();
          }}
          className="form__action form__action--danger"
        >
          {t('dialogs.deleteScope.series')}
        </button>
      </div>
    </Modal>
  );
}
