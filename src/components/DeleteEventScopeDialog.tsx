import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../api/types';
import { useCancellationChoice } from '../state/useCancellationChoice';
import { Modal } from './Modal';

/**
 * "Delete one occurrence vs the whole series" dialog for the keyboard shortcut
 * path in the calendar views (the editor does the same via its scope radios).
 *
 * When the event is a meeting the connected account ORGANIZES (attendees on a
 * scheduling-capable provider), picking a scope opens a second step: "cancel &
 * notify the attendees" vs "remove without notifying" — the same choice the
 * editor offers on delete. For everyone else (an attendee's copy, a non-meeting
 * event, a local calendar) the scope buttons remove silently in one step.
 *
 * Cancel takes initial focus on every step — mashing Enter never deletes or
 * emails anything by accident; the destructive choices come last.
 */
export interface DeleteEventScopeDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Event title shown inside the prompt. */
  title: string;
  /** The event being removed — drives the organizer/attendee cancel-choice. */
  event: CalendarEvent | null;
  /** Drop just this occurrence; `sendCancellations` emails attendees a
   *  per-occurrence cancellation (organizer only). */
  onOccurrence: (sendCancellations: boolean) => void;
  /** Drop the whole series; `sendCancellations` emails attendees a cancellation
   *  for the meeting (organizer only). */
  onSeries: (sendCancellations: boolean) => void;
}

export function DeleteEventScopeDialog({
  isOpen,
  onClose,
  title,
  event,
  onOccurrence,
  onSeries,
}: DeleteEventScopeDialogProps) {
  const { t } = useTranslation();
  const { offersChoice } = useCancellationChoice(event);
  const cancelRef = useRef<HTMLButtonElement>(null);

  // 'scope' = occurrence vs series; 'notify' = (organizer only) cancel+notify
  // vs remove-silently for the chosen scope.
  const [step, setStep] = useState<'scope' | 'notify'>('scope');
  const [scope, setScope] = useState<'occurrence' | 'series'>('occurrence');

  // Always reopen on the scope step, and take cancel-first focus on each step.
  useEffect(() => {
    if (!isOpen) return;
    setStep('scope');
  }, [isOpen]);
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => cancelRef.current?.focus());
  }, [isOpen, step]);

  const finish = (which: 'occurrence' | 'series', sendCancellations: boolean) => {
    (which === 'occurrence' ? onOccurrence : onSeries)(sendCancellations);
    onClose();
  };

  const pickScope = (which: 'occurrence' | 'series') => {
    // Organizer of a meeting with attendees → ask whether to notify; otherwise
    // remove silently in one step.
    if (offersChoice) {
      setScope(which);
      setStep('notify');
    } else {
      finish(which, false);
    }
  };

  const notifyMessageKey =
    scope === 'occurrence'
      ? 'dialogs.event.cancelChoice.occurrenceMessage'
      : 'dialogs.event.cancelChoice.message';
  const notifyConfirmKey =
    scope === 'occurrence'
      ? 'dialogs.event.cancelChoice.cancelOccurrence'
      : 'dialogs.event.cancelChoice.cancelMeeting';

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={
        step === 'scope'
          ? t('dialogs.deleteScope.title')
          : t('dialogs.event.cancelChoice.title')
      }
      className="modal--confirm modal--confirm-wide"
      dismissOnBackdrop={false}
    >
      {step === 'scope' ? (
        <>
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
              onClick={() => pickScope('occurrence')}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.occurrence')}
            </button>
            <button
              type="button"
              onClick={() => pickScope('series')}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.series')}
            </button>
          </div>
        </>
      ) : (
        <>
          <p className="form__message">{t(notifyMessageKey, { title })}</p>
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
              onClick={() => finish(scope, false)}
              className="form__action form__action--danger"
            >
              {t('dialogs.event.cancelChoice.removeSilently')}
            </button>
            <button
              type="button"
              onClick={() => finish(scope, true)}
              className="form__action form__action--danger"
            >
              {t(notifyConfirmKey)}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}
