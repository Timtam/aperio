import { useId, useRef } from 'react';

import type { WeekdayQuestionText } from '@aperio/shared';

import { Modal } from './Modal';

export interface WeekdayQuestionDialogProps {
  isOpen: boolean;
  /** The question as `weekdayQuestion` words it (138a). */
  question: WeekdayQuestionText;
  /** "Zeitzone nicht ändern", Escape or the close button: nothing changes. */
  onKeep: () => void;
  /** One of the answers the core can carry out. */
  onAnswer: (answer: 'rule' | 'now') => void;
}

/**
 * The weekday question on the desktop (DESIGN-series-time-zone.md, "Die
 * Wochentag-Frage (15a)", in the wording and order of 138a): asked after a
 * zone was chosen for a stored series whose start falls on another day on the
 * new clock.
 *
 * Not a `ConfirmDialog`: that one closes after every action and styles its
 * confirm button as dangerous, and none of these choices is. The buttons are
 * "Zeitzone nicht ändern" first, with the first focus, then the rule's answer
 * and the answer where the events fall now; an answer the core refuses has no
 * button, and the message says why. The message is every button's
 * description, so it is read with whichever button has the focus. The caller
 * closes the dialog; the Modal then puts the focus back on what opened it,
 * the zone button.
 */
export function WeekdayQuestionDialog({ isOpen, question, onKeep, onAnswer }: WeekdayQuestionDialogProps) {
  const keepRef = useRef<HTMLButtonElement>(null);
  const messageId = useId();
  return (
    <Modal
      isOpen={isOpen}
      onClose={onKeep}
      title={question.title}
      initialFocusRef={keepRef}
      className="modal--confirm-wide"
      dismissOnBackdrop={false}
    >
      <p id={messageId} className="form__message">
        {question.message}
      </p>
      <div className="form__actions form__actions--stacked">
        <button
          ref={keepRef}
          type="button"
          onClick={onKeep}
          className="form__action"
          aria-describedby={messageId}
        >
          {question.keep}
        </button>
        {question.answers.map(({ answer, label }) => (
          <button
            key={answer}
            type="button"
            onClick={() => onAnswer(answer)}
            className="form__action"
            aria-describedby={messageId}
          >
            {label}
          </button>
        ))}
      </div>
    </Modal>
  );
}
