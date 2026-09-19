import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../api/types';
import { useCancellationChoice } from '../state/useCancellationChoice';
import { ConfirmDialog } from './ConfirmDialog';

/**
 * The confirmation before a view deletes a whole event (the Delete key in the
 * Week, Day, Month and Agenda views), by the rule the editor, the chip menu
 * and the mobile delete share (`useCancellationChoice`, decisions 70a, 80a):
 *
 * - a plain event, someone else's meeting, a local calendar: "Delete X? This
 *   cannot be undone.";
 * - a meeting the account organizes, on a provider that can delete silently:
 *   "Cancel meeting" (the attendees are notified) or "Remove without
 *   notifying";
 * - a provider that cancels it for the attendees whatever the request says
 *   (iCloud, Microsoft 365): the sentence that says who informs them, and one
 *   button that cancels.
 *
 * `onDelete` gets whether to send the cancellations.
 */
export function DeleteEventConfirm({
  event,
  onClose,
  onDelete,
}: {
  event: CalendarEvent | null;
  onClose: () => void;
  onDelete: (event: CalendarEvent, sendCancellations: boolean) => void;
}) {
  const { t } = useTranslation();
  const { offersChoice, alwaysNotifies, sentence } = useCancellationChoice(event);
  const title = event?.title ?? '';
  const asks = offersChoice || alwaysNotifies;
  return (
    <ConfirmDialog
      isOpen={event !== null}
      onClose={onClose}
      onConfirm={() => {
        if (event) onDelete(event, asks);
      }}
      title={t(asks ? 'dialogs.event.cancelChoice.title' : 'dialogs.confirm.deleteEventTitle')}
      message={
        alwaysNotifies
          ? t('dialogs.event.cancelChoice.alwaysMessage', {
              title,
              sentence: t(sentence.key, sentence.values),
            })
          : offersChoice
            ? t('dialogs.event.cancelChoice.message', { title })
            : t('dialogs.confirm.deleteEventMessage', { title })
      }
      confirmLabel={asks ? t('dialogs.event.cancelChoice.cancelMeeting') : undefined}
      extraActions={
        offersChoice
          ? [
              {
                label: t('dialogs.event.cancelChoice.removeSilently'),
                onClick: () => {
                  if (event) onDelete(event, false);
                },
                danger: true,
              },
            ]
          : undefined
      }
    />
  );
}
