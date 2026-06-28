import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';

import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/dialogStateContext';
import { Modal } from './Modal';

/**
 * Day-activation chooser — "Termin oder Aufgabe?".
 *
 * Double-clicking (or pressing Enter on) an empty calendar day used to jump
 * straight into the event editor. It now opens this small chooser so the same
 * gesture can create either an event or a task. Picking one routes to the
 * matching quick-add dialog, anchored to the activated day (`defaultDate`),
 * from which "weitere Details" expands to the full editor.
 *
 * Rendered as a focus-trapping Modal (not a positioned popup menu) so keyboard
 * and screen-reader users get a robust, predictable two-choice prompt — the
 * same primitive carries to mobile as an AppDialog.
 */
export function CreateChooserDialog({
  isOpen,
  onClose,
  defaultDate,
}: {
  isOpen: boolean;
  onClose: () => void;
  /** YYYY-MM-DD of the activated calendar day, carried into the chosen
   *  quick-add so the new item lands on that day. */
  defaultDate?: string;
}) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { openQuickAdd, openQuickAddTask } = useDialogState();

  const dayLabel = (() => {
    if (!defaultDate) return '';
    try {
      return fmt.format(new Date(`${defaultDate}T00:00:00`), 'PPP');
    } catch {
      return defaultDate;
    }
  })();

  // Pop this chooser first, then push the chosen quick-add. Same hand-off
  // pattern the quick-add dialogs use for "weitere Details".
  const chooseEvent = useCallback(() => {
    onClose();
    openQuickAdd({ defaultDate });
  }, [onClose, openQuickAdd, defaultDate]);

  const chooseTask = useCallback(() => {
    onClose();
    openQuickAddTask({ defaultDate });
  }, [onClose, openQuickAddTask, defaultDate]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.createChooser.title')}
      className="modal--form modal--narrow create-chooser"
      dismissOnBackdrop={false}
    >
      {dayLabel && (
        <p className="create-chooser__day">
          {t('dialogs.createChooser.onDay', { date: dayLabel })}
        </p>
      )}
      <div className="create-chooser__choices">
        <button
          type="button"
          onClick={chooseEvent}
          className="create-chooser__choice"
        >
          {t('dialogs.createChooser.event')}
        </button>
        <button
          type="button"
          onClick={chooseTask}
          className="create-chooser__choice"
        >
          {t('dialogs.createChooser.task')}
        </button>
      </div>
      <div className="form__actions">
        <button type="button" onClick={onClose} className="form__action">
          {t('dialogs.cancel')}
        </button>
      </div>
    </Modal>
  );
}
