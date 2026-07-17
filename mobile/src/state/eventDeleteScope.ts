import { Alert } from 'react-native';

import { occurrenceIsoOf, seriesIdOf } from '@aperio/shared';

import { addEventExdate, CalendarEvent, deleteEvent } from '../api/calendar';
import { resolveCalendarUserEmail } from './currentUserEmail';
import { deleteThisAndFuture } from './deleteSeriesFromOccurrence';

/** Lower-case, `mailto:`-stripped form for comparing addresses. */
function normalizeEmail(value: string | null | undefined): string {
  if (!value) return '';
  return value.trim().replace(/^mailto:/i, '').toLowerCase();
}

// Shared event-delete confirmation with recurrence scope — the mobile analogue
// of the desktop EventDialog's delete-scope choice. A concrete occurrence of a
// recurring series offers "This occurrence only" (append its instant to the
// master's EXDATE via add_event_exdate) vs "Whole series" (delete the master); a
// single event gets a plain delete. Used by every calendar surface (the shared
// CalendarDayList for Week+Month, plus EventsScreen + AgendaScreen) so the scope
// logic lives in one place.

type Tr = (key: string, vars?: Record<string, unknown>) => string;

/** Pop the delete-confirm for `ev`; on a successful mutation calls
 *  `onSuccess(announceMessage)`, on failure `onError(message)`.
 *
 *  When `supportsScheduling` is true (the event's calendar is on a
 *  scheduling-capable provider) AND the event has attendees, this resolves
 *  whether the connected account ORGANIZES the meeting (via
 *  `calendarCurrentUserEmail`); if so, a whole-event/series delete becomes a
 *  three-way choice — cancel + notify attendees / remove without notifying /
 *  keep. An attendee's copy, a non-meeting event, or a non-scheduling provider
 *  gets a plain delete (no cancellation). A single occurrence is always a local
 *  EXDATE that never notifies. Every calendar surface routes here so the logic
 *  lives in one place. */
export function confirmDeleteEvent(
  ev: CalendarEvent,
  t: Tr,
  onSuccess: (message: string) => void,
  onError: (message: string) => void,
  opts: { supportsScheduling?: boolean } = {},
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

  // Delete the whole event/series, optionally emailing a cancellation to the
  // attendees. Deleting a single occurrence (EXDATE) never notifies.
  const deleteWith = (sendCancellations: boolean) =>
    run(
      () => deleteEvent(series, ev.calendar_id, sendCancellations),
      sendCancellations
        ? t('dialogs.event.meetingCancelled', { title: ev.title })
        : t('dialogs.event.deleted', { title: ev.title }),
    );

  // Remove just this occurrence: silent local skip, or (organizer) a per-
  // occurrence cancellation that emails the attendees.
  const removeOccurrence = (sendCancellations: boolean) =>
    run(
      () =>
        addEventExdate(series, occurrence!, ev.calendar_id, sendCancellations),
      sendCancellations
        ? t('dialogs.event.occurrenceCancelled', { title: ev.title })
        : t('dialogs.event.occurrenceDeleted', { title: ev.title }),
    );

  // Remove this occurrence AND all following ones (truncate the series).
  const removeThisAndFuture = (sendCancellations: boolean) =>
    run(
      () => deleteThisAndFuture(ev, occurrence!, sendCancellations),
      sendCancellations
        ? t('dialogs.event.thisAndFutureCancelled', { title: ev.title })
        : t('dialogs.event.thisAndFutureDeleted', { title: ev.title }),
    );

  // Recurring-occurrence delete. For a meeting the account ORGANIZES, ALL four
  // choices are explicit buttons in ONE prompt — cancel-and-notify vs remove-
  // silently, for this occurrence and for the whole series — so there is never a
  // hidden second step and every button spells out whether an email goes out.
  // Everyone else gets the plain two-scope delete.
  const occurrenceAlert = (organizer: boolean) =>
    organizer
      ? Alert.alert(
          t('dialogs.deleteScope.title'),
          t('dialogs.deleteScope.organizerMessage', { title: ev.title }),
          [
            { text: t('mobile.cancel'), style: 'cancel' },
            {
              text: t('dialogs.deleteScope.occurrenceNotify'),
              style: 'destructive',
              onPress: () => removeOccurrence(true),
            },
            {
              text: t('dialogs.deleteScope.occurrenceSilent'),
              style: 'destructive',
              onPress: () => removeOccurrence(false),
            },
            {
              text: t('dialogs.deleteScope.thisAndFutureNotify'),
              style: 'destructive',
              onPress: () => removeThisAndFuture(true),
            },
            {
              text: t('dialogs.deleteScope.thisAndFutureSilent'),
              style: 'destructive',
              onPress: () => removeThisAndFuture(false),
            },
            {
              text: t('dialogs.deleteScope.seriesNotify'),
              style: 'destructive',
              onPress: () => deleteWith(true),
            },
            {
              text: t('dialogs.deleteScope.seriesSilent'),
              style: 'destructive',
              onPress: () => deleteWith(false),
            },
          ],
        )
      : Alert.alert(
          t('dialogs.confirm.deleteEventTitle'),
          t('dialogs.confirm.deleteEventMessage', { title: ev.title }),
          [
            { text: t('mobile.cancel'), style: 'cancel' },
            {
              text: t('dialogs.event.scope.occurrence'),
              onPress: () => removeOccurrence(false),
            },
            {
              text: t('dialogs.event.scope.thisAndFuture'),
              onPress: () => removeThisAndFuture(false),
            },
            {
              // A whole-series delete can still email a cancellation; the
              // adapters tolerate send-cancellations from a non-organizer (fall
              // back to a plain delete), so `attendees > 0` is safe here.
              text: t('dialogs.event.scope.series'),
              style: 'destructive',
              onPress: () => deleteWith(ev.attendees.length > 0),
            },
          ],
        );

  const choiceAlert = () =>
    Alert.alert(
      t('dialogs.event.cancelChoice.title'),
      t('dialogs.event.cancelChoice.message', { title: ev.title }),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.event.cancelChoice.removeSilently'),
          style: 'destructive',
          onPress: () => deleteWith(false),
        },
        {
          text: t('dialogs.event.cancelChoice.cancelMeeting'),
          style: 'destructive',
          onPress: () => deleteWith(true),
        },
      ],
    );

  const plainAlert = () =>
    Alert.alert(
      t('dialogs.confirm.deleteEventTitle'),
      t('dialogs.confirm.deleteEventMessage', { title: ev.title }),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.event.delete'),
          style: 'destructive',
          onPress: () => deleteWith(false),
        },
      ],
    );

  // Only a meeting we ORGANIZE on a scheduling provider offers the
  // notify/silent choice (for the whole series OR a single occurrence).
  // Resolve "who am I" lazily at delete time (a cheap host read).
  const organizerGated = ev.attendees.length > 0 && opts.supportsScheduling;

  if (occurrence != null) {
    if (organizerGated) {
      void resolveCalendarUserEmail(ev.calendar_id)
        .then((me) => {
          const isOrganizer =
            !!me && normalizeEmail(ev.organizer) === normalizeEmail(me);
          occurrenceAlert(isOrganizer);
        })
        .catch(() => occurrenceAlert(false));
      return;
    }
    occurrenceAlert(false);
    return;
  }
  if (organizerGated) {
    void resolveCalendarUserEmail(ev.calendar_id)
      .then((me) => {
        const isOrganizer =
          !!me && normalizeEmail(ev.organizer) === normalizeEmail(me);
        if (isOrganizer) choiceAlert();
        else plainAlert();
      })
      .catch(() => plainAlert());
    return;
  }
  plainAlert();
}
