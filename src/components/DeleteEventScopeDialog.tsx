import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../api/types';
import { useCancellationChoice } from '../state/useCancellationChoice';
import { Modal } from './Modal';

/**
 * "Delete one occurrence vs the whole series" dialog for the keyboard shortcut
 * path in the calendar views (the editor does the same via its scope radios).
 *
 * When the event is a meeting the connected account ORGANIZES (attendees on a
 * scheduling-capable provider, via `useCancellationChoice`), ALL of the choices
 * are shown as explicit buttons in ONE step — cancel-and-notify vs remove-
 * silently, for this occurrence and for the whole series — so there is never a
 * hidden second prompt: every button says exactly what it will do (whether an
 * email goes out). For everyone else (an attendee's copy, a non-meeting event, a
 * local calendar) it's the plain two-scope delete.
 *
 * Cancel takes initial focus — mashing Enter never deletes or emails anything by
 * accident; the destructive choices come after it in tab order.
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

  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => cancelRef.current?.focus());
  }, [isOpen]);

  const finish = (which: 'occurrence' | 'series', sendCancellations: boolean) => {
    (which === 'occurrence' ? onOccurrence : onSeries)(sendCancellations);
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
      <div className="form__actions form__actions--stacked">
        <button
          ref={cancelRef}
          type="button"
          onClick={onClose}
          className="form__action"
        >
          {t('dialogs.deleteScope.cancel')}
        </button>
        {offersChoice ? (
          <>
            <button
              type="button"
              onClick={() => finish('occurrence', true)}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.occurrenceNotify')}
            </button>
            <button
              type="button"
              onClick={() => finish('occurrence', false)}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.occurrenceSilent')}
            </button>
            <button
              type="button"
              onClick={() => finish('series', true)}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.seriesNotify')}
            </button>
            <button
              type="button"
              onClick={() => finish('series', false)}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.seriesSilent')}
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              onClick={() => finish('occurrence', false)}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.occurrence')}
            </button>
            <button
              type="button"
              onClick={() => finish('series', false)}
              className="form__action form__action--danger"
            >
              {t('dialogs.deleteScope.series')}
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}
