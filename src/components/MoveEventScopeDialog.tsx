import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * "Move only this occurrence vs the whole series vs cancel" dialog for
 * dropping a RECURRING event onto another day (week / month planner
 * drag-and-drop). Mirrors `DeleteEventScopeDialog`'s shape — a quick
 * three-button modal beats a radio group for the interaction that
 * interrupts a drop — but with non-destructive button styling, since
 * moving is reversible.
 *
 * Cancel takes initial focus so an accidental drop + Enter changes
 * nothing.
 */
export interface MoveEventScopeDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Event title shown inside the prompt. */
  title: string;
  /** Detach just this occurrence onto the target day. */
  onOccurrence: () => void;
  /** Re-anchor the whole series on the target day. */
  onSeries: () => void;
}

export function MoveEventScopeDialog({
  isOpen,
  onClose,
  title,
  onOccurrence,
  onSeries,
}: MoveEventScopeDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => cancelRef.current?.focus());
  }, [isOpen]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.moveScope.title')}
      className="modal--confirm modal--confirm-wide"
      dismissOnBackdrop={false}
    >
      <p className="form__message">
        {t('dialogs.moveScope.message', { title })}
      </p>
      <div className="form__actions">
        <button
          ref={cancelRef}
          type="button"
          onClick={onClose}
          className="form__action"
        >
          {t('dialogs.moveScope.cancel')}
        </button>
        <button
          type="button"
          onClick={() => {
            onOccurrence();
            onClose();
          }}
          className="form__action"
        >
          {t('dialogs.moveScope.occurrence')}
        </button>
        <button
          type="button"
          onClick={() => {
            onSeries();
            onClose();
          }}
          className="form__action form__action--primary"
        >
          {t('dialogs.moveScope.series')}
        </button>
      </div>
    </Modal>
  );
}
