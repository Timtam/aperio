import { useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { ShiftRefusal } from '../state/moveActions';
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
  /** Move the whole series by the same distance. */
  onSeries: () => void;
  /** Set when moving the whole series was refused: the prompt says why and
   *  offers only this occurrence. */
  refused?: ShiftRefusal | null;
}

export function MoveEventScopeDialog({
  isOpen,
  onClose,
  title,
  onOccurrence,
  onSeries,
  refused = null,
}: MoveEventScopeDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const messageId = useId();

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
      // The message names the event and, after a refusal, why the series
      // cannot move. As a static <p> in the role="application" body it is
      // unreachable to NVDA, so it is the dialog's description and is read
      // when the dialog opens.
      describedById={messageId}
    >
      <p id={messageId} className="form__message">
        {refused
          ? t('dialogs.moveScope.refusedMessage', {
              title,
              reason: t(`dialogs.moveScope.refusal.${refused}`),
            })
          : t('dialogs.moveScope.message', { title })}
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
        {refused ? null : (
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
        )}
      </div>
    </Modal>
  );
}
