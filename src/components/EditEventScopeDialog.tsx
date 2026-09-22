import { useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * Outlook-style "edit this occurrence vs the whole series" prompt, shown when
 * the user opens a RECURRING event's editor. Mirrors `MoveEventScopeDialog`'s
 * shape. "This occurrence" and "this and all following" hand off to the event
 * editor locked to that scope; "whole series" opens the editor on the series
 * itself. The choice is made up front instead of via a radio group buried in
 * the form — which a screen-reader user could miss and edit the whole series by
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
  /** Open the editor on the whole series. */
  onSeries: () => void;
  /** How often the whole series failed to load. The prompt stays and says so;
   *  each new count is announced again. */
  seriesLoadFailed?: number;
  /** Which choice's load failed: "this and all following" loads the series
   *  too, for an occurrence the provider keeps (126). The failure describes
   *  that button, where focus stays. */
  seriesLoadFailedScope?: 'series' | 'occurrence' | 'this_and_future';
}

export function EditEventScopeDialog({
  isOpen,
  onClose,
  title,
  onOccurrence,
  onThisAndFuture,
  onSeries,
  seriesLoadFailed = 0,
  seriesLoadFailedScope = 'series',
}: EditEventScopeDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const msgId = useId();
  const failureId = useId();

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
      {seriesLoadFailed > 0 && (
        // A fresh node for every failure: an unchanged alert is not announced
        // again, and a retry that failed too would pass in silence. It also
        // describes the button that failed, where focus stays, so it can be
        // read again.
        <p key={seriesLoadFailed} id={failureId} role="alert" className="form__error">
          {t('dialogs.editScope.seriesLoadFailed', { title })}
        </p>
      )}
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
          aria-describedby={
            seriesLoadFailed > 0 && seriesLoadFailedScope === 'this_and_future'
              ? failureId
              : undefined
          }
        >
          {t('dialogs.editScope.thisAndFuture')}
        </button>
        <button
          type="button"
          onClick={onSeries}
          className="form__action form__action--primary"
          aria-describedby={
            seriesLoadFailed > 0 && seriesLoadFailedScope !== 'this_and_future'
              ? failureId
              : undefined
          }
        >
          {t('dialogs.editScope.series')}
        </button>
      </div>
    </Modal>
  );
}
