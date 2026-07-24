import { useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * Outlook-style "edit this occurrence vs the whole series" prompt, shown when
 * the user opens a RECURRING event's editor. Mirrors `MoveEventScopeDialog`'s
 * shape; picking a scope hands off to the event editor (locked to that scope),
 * so the choice is made up front instead of via a radio group buried in the
 * form — which a screen-reader user could miss and edit the whole series by
 * accident.
 *
 * Cancel takes initial focus so a stray Enter changes nothing.
 */
export interface EditEventScopeDialogProps {
  isOpen: boolean;
  /** Dismiss without opening the editor (focus returns to the opener). */
  onClose: () => void;
  /** Event title shown inside the prompt. */
  title: string;
  /** Open the editor scoped to just this occurrence. */
  onOccurrence: () => void;
  /** Open the editor scoped to this occurrence and all following ones. */
  onThisAndFuture: () => void;
  /** Open the editor scoped to the whole series. */
  onSeries: () => void;
}

export function EditEventScopeDialog({
  isOpen,
  onClose,
  title,
  onOccurrence,
  onThisAndFuture,
  onSeries,
}: EditEventScopeDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const msgId = useId();

  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => cancelRef.current?.focus());
  }, [isOpen]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.editScope.title')}
      className="modal--confirm modal--confirm-wide"
      dismissOnBackdrop={false}
    >
      {/* The message names WHICH recurring event is being edited. It lives in
          Modal's role="application" body, where a static <p> is invisible to
          focus-mode traversal — so describe the initially-focused Cancel button
          with it, and NVDA speaks it the instant the dialog opens. */}
      <p id={msgId} className="form__message">
        {t('dialogs.editScope.message', { title })}
      </p>
      <div className="form__actions">
        <button
          ref={cancelRef}
          type="button"
          onClick={onClose}
          className="form__action"
          aria-describedby={msgId}
        >
          {t('dialogs.editScope.cancel')}
        </button>
        <button type="button" onClick={onOccurrence} className="form__action">
          {t('dialogs.editScope.occurrence')}
        </button>
        <button
          type="button"
          onClick={onThisAndFuture}
          className="form__action"
        >
          {t('dialogs.editScope.thisAndFuture')}
        </button>
        <button
          type="button"
          onClick={onSeries}
          className="form__action form__action--primary"
        >
          {t('dialogs.editScope.series')}
        </button>
      </div>
    </Modal>
  );
}
