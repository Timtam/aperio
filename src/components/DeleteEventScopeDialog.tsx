import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../api/types';
import { useCancellationChoice } from '../state/useCancellationChoice';
import { Modal } from './Modal';

/**
 * "Delete this occurrence / this and all following / the whole series" dialog for
 * the keyboard shortcut path in the calendar views.
 *
 * When the event is a meeting the connected account ORGANIZES (attendees on a
 * scheduling-capable provider, via `useCancellationChoice`), a **Notify
 * attendees / Remove without notifying** radio group sits above the scope
 * buttons (default: notify) — so the notify choice is made once, transparently,
 * and each scope button applies it. Attendee copies / non-meetings / local
 * events show the scope buttons alone (silent). Cancel takes initial focus for
 * those; for the organizer form the radio group is focused first (non-
 * destructive) so the choice is surfaced before the scope buttons.
 */
export interface DeleteEventScopeDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Event title shown inside the prompt. */
  title: string;
  /** The event being removed — drives the organizer/attendee notify choice. */
  event: CalendarEvent | null;
  /** Drop just this occurrence; `sendCancellations` emails attendees. */
  onOccurrence: (sendCancellations: boolean) => void;
  /** Drop this occurrence and every later one (truncate the series);
   *  `sendCancellations` emails attendees. */
  onThisAndFuture: (sendCancellations: boolean) => void;
  /** Drop the whole series; `sendCancellations` emails attendees. */
  onSeries: (sendCancellations: boolean) => void;
}

export function DeleteEventScopeDialog({
  isOpen,
  onClose,
  title,
  event,
  onOccurrence,
  onThisAndFuture,
  onSeries,
}: DeleteEventScopeDialogProps) {
  const { t } = useTranslation();
  const { offersChoice } = useCancellationChoice(event);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const notifyRef = useRef<HTMLInputElement>(null);

  // Notify default = on (the common intent when cancelling a meeting you own).
  const [notify, setNotify] = useState(true);
  useEffect(() => {
    if (isOpen) setNotify(true);
  }, [isOpen]);

  // Focus the radio first when it's shown (non-destructive), else the cancel
  // button — never a scope (delete) button.
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => {
      if (offersChoice) notifyRef.current?.focus();
      else cancelRef.current?.focus();
    });
  }, [isOpen, offersChoice]);

  const run = (fn: (send: boolean) => void) => {
    fn(offersChoice ? notify : false);
    onClose();
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.deleteScope.title')}
      className="modal--confirm modal--confirm-wide"
      dismissOnBackdrop={false}
    >
      <p className="form__message">
        {t(
          offersChoice
            ? 'dialogs.deleteScope.organizerMessage'
            : 'dialogs.deleteScope.message',
          { title },
        )}
      </p>
      {offersChoice && (
        <fieldset className="form__field">
          <legend className="form__label">
            {t('dialogs.deleteScope.notifyLegend')}
          </legend>
          <label className="form__field form__field--inline">
            <input
              ref={notifyRef}
              type="radio"
              name="delete-scope-notify"
              checked={notify}
              onChange={() => setNotify(true)}
            />
            <span>{t('dialogs.deleteScope.notifyAttendees')}</span>
          </label>
          <label className="form__field form__field--inline">
            <input
              type="radio"
              name="delete-scope-notify"
              checked={!notify}
              onChange={() => setNotify(false)}
            />
            <span>{t('dialogs.deleteScope.notifySilent')}</span>
          </label>
        </fieldset>
      )}
      <div className="form__actions form__actions--stacked">
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
          onClick={() => run(onOccurrence)}
          className="form__action form__action--danger"
        >
          {t('dialogs.deleteScope.occurrence')}
        </button>
        <button
          type="button"
          onClick={() => run(onThisAndFuture)}
          className="form__action form__action--danger"
        >
          {t('dialogs.deleteScope.thisAndFuture')}
        </button>
        <button
          type="button"
          onClick={() => run(onSeries)}
          className="form__action form__action--danger"
        >
          {t('dialogs.deleteScope.series')}
        </button>
      </div>
    </Modal>
  );
}
