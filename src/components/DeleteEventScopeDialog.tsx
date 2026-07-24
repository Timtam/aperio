import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
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
  const announce = useAnnouncer();
  const { offersChoice } = useCancellationChoice(event);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const notifyRef = useRef<HTMLInputElement>(null);
  const messageId = useId();
  // Live mirror of offersChoice so the open-focus effect can read the value AT
  // OPEN without depending on it (which would re-run and STEAL focus onto the
  // notify radio if the async organizer check resolves mid-life).
  const offersChoiceRef = useRef(offersChoice);
  offersChoiceRef.current = offersChoice;

  // Notify default = on (the common intent when cancelling a meeting you own).
  const [notify, setNotify] = useState(true);
  useEffect(() => {
    if (isOpen) setNotify(true);
  }, [isOpen]);

  // Focus ONCE per open — the radio when the organizer choice is already known
  // at open (non-destructive), else Cancel; never a scope (delete) button. Keyed
  // on isOpen only (reads the ref), so a late organizer-check resolve grows the
  // dialog but never yanks focus onto a control the user hasn't heard of.
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => {
      if (offersChoiceRef.current) notifyRef.current?.focus();
      else cancelRef.current?.focus();
    });
  }, [isOpen]);

  // The organizer check resolves async, so the notify section appears a beat
  // after open. Announce that reveal (a false→true transition while open) so the
  // grown dialog isn't a silent surprise; focus deliberately stays put (see
  // above), and the user can Tab to the newly announced radios. `prev` starts at
  // the current value, so a section already present on the first render (were
  // the check ever synchronous) would not announce.
  const prevOffersChoiceRef = useRef(offersChoice);
  useEffect(() => {
    if (!isOpen) {
      prevOffersChoiceRef.current = offersChoice;
      return;
    }
    if (offersChoice && !prevOffersChoiceRef.current) {
      announce(t('dialogs.deleteScope.notifyRevealed'));
    }
    prevOffersChoiceRef.current = offersChoice;
  }, [isOpen, offersChoice, announce, t]);

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
      // The message names WHICH event is being deleted; the generic title does
      // not. As a static <p> in the role="application" body it was unreachable
      // to NVDA — wire it as the dialog's aria-describedby so it is read in the
      // open announcement.
      describedById={messageId}
    >
      <p id={messageId} className="form__message">
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
