import { Alert } from 'react-native';

import { occurrenceIsoOf, seriesIdOf } from '@aperio/shared';

import { addEventExdate, CalendarEvent, deleteEvent } from '../api/calendar';

// Shared event-delete confirmation with recurrence scope — the mobile analogue
// of the desktop EventDialog's delete-scope choice. A concrete occurrence of a
// recurring series offers "This occurrence only" (append its instant to the
// master's EXDATE via add_event_exdate) vs "Whole series" (delete the master); a
// single event gets a plain delete. Used by every calendar surface (the shared
// CalendarDayList for Week+Month, plus EventsScreen + AgendaScreen) so the scope
// logic lives in one place.

type Tr = (key: string, vars?: Record<string, unknown>) => string;

/** Pop the delete-confirm for `ev`; on a successful mutation calls
 *  `onSuccess(announceMessage)`, on failure `onError(message)`. */
export function confirmDeleteEvent(
  ev: CalendarEvent,
  t: Tr,
  onSuccess: (message: string) => void,
  onError: (message: string) => void,
): void {
  const series = seriesIdOf(ev);
  // Non-null only for an expanded occurrence of a recurring series.
  const occurrence = occurrenceIsoOf(ev);

  const run = (fn: () => Promise<void>, message: string) => {
    void (async () => {
      try {
        await fn();
        onSuccess(message);
      } catch (err) {
        onError(err instanceof Error ? err.message : String(err));
      }
    })();
  };

  // Notify attendees only when the meeting has any (desktop parity — the chip
  // menu passes send-cancellations = attendees.length > 0). Deleting a single
  // occurrence (EXDATE) doesn't notify; only the master/single delete does.
  const deleteSeries = () =>
    run(
      () => deleteEvent(series, ev.calendar_id, ev.attendees.length > 0),
      t('dialogs.event.deleted', { title: ev.title }),
    );

  if (occurrence != null) {
    Alert.alert(
      t('dialogs.confirm.deleteEventTitle'),
      t('dialogs.confirm.deleteEventMessage', { title: ev.title }),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.event.scope.occurrence'),
          onPress: () =>
            run(
              () => addEventExdate(series, occurrence, ev.calendar_id),
              t('dialogs.event.occurrenceDeleted', { title: ev.title }),
            ),
        },
        {
          text: t('dialogs.event.scope.series'),
          style: 'destructive',
          onPress: deleteSeries,
        },
      ],
    );
  } else {
    Alert.alert(
      t('dialogs.confirm.deleteEventTitle'),
      t('dialogs.confirm.deleteEventMessage', { title: ev.title }),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        { text: t('dialogs.event.delete'), style: 'destructive', onPress: deleteSeries },
      ],
    );
  }
}
