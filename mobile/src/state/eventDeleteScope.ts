import { Alert } from 'react-native';

import {
  cancellationNotice,
  declineSentence,
  eventWriteErrorMessage,
  invitationLocked,
  notifierSentence,
  silentSentence,
  occurrenceIsoOf,
  seriesIdOf,
  type NoticeCalendar,
} from '@aperio/shared';

import { showEventScopeDialog } from './eventScopeDialog';
import { addEventExdate, CalendarEvent, deleteEvent } from '../api/calendar';
import { deleteThisAndFuture } from './deleteSeriesFromOccurrence';

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
 *  `opts.calendar` is the event's calendar row. The shared rule
 *  (`cancellationNotice`, the desktop's too) decides what the delete asks:
 *  for a meeting the account ORGANIZES, with attendees, on a
 *  scheduling-capable provider, a whole-event/series delete becomes a
 *  three-way choice — cancel + notify attendees / remove without notifying /
 *  keep — where the provider can delete silently, and a confirmation that
 *  says who informs the attendees where it always does (decision 80a). An
 *  attendee's copy, a non-meeting event, or a non-scheduling provider gets a
 *  plain delete (no cancellation). Every calendar surface routes here so the
 *  logic lives in one place. */
export function confirmDeleteEvent(
  ev: CalendarEvent,
  t: Tr,
  onSuccess: (message: string) => void,
  onError: (message: string) => void,
  opts: { calendar?: NoticeCalendar | null } = {},
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
        // The one place every mobile delete path reports from: a refusal is
        // said in words, not as "forbidden: reply-only-invitation: …".
        onError(eventWriteErrorMessage(err, t));
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

  // Recurring-occurrence delete. Rendered as an in-app dialog (NOT Alert): the
  // organizer form pairs a notify/silent radio (default: notify) with three scope
  // buttons — this occurrence / this and all following / whole series — so the
  // notify choice is made once and every scope applies it. Everyone else gets the
  // three scope buttons alone (silent). A native Alert can't carry this many
  // options — Android keeps only its first three buttons — so whole-series
  // delete would be unreachable; the dialog renders them all, matching the
  // desktop DeleteEventScopeDialog.
  const occurrenceAlert = (organizer: boolean) =>
    organizer
      ? showEventScopeDialog({
          title: t('dialogs.deleteScope.title'),
          message: t('dialogs.deleteScope.organizerMessage', { title: ev.title }),
          cancelLabel: t('dialogs.deleteScope.cancel'),
          notify: {
            legend: t('dialogs.deleteScope.notifyLegend'),
            notifyLabel: t('dialogs.deleteScope.notifyAttendees'),
            silentLabel: t('dialogs.deleteScope.notifySilent'),
          },
          options: [
            {
              key: 'occurrence',
              label: t('dialogs.deleteScope.occurrence'),
              destructive: true,
              run: (send) => removeOccurrence(send),
            },
            {
              key: 'thisAndFuture',
              label: t('dialogs.deleteScope.thisAndFuture'),
              destructive: true,
              run: (send) => removeThisAndFuture(send),
            },
            {
              key: 'series',
              label: t('dialogs.deleteScope.series'),
              destructive: true,
              run: (send) => deleteWith(send),
            },
          ],
        })
      : showEventScopeDialog({
          title: t('dialogs.confirm.deleteEventTitle'),
          message: `${t('dialogs.confirm.deleteEventMessage', { title: ev.title })}${
            invitationLocked(opts.calendar, ev) || ev.scheduling_silenced === true
              ? ` ${deleteSentence()}`
              : ''
          }`,
          cancelLabel: t('mobile.cancel'),
          options: [
            {
              key: 'occurrence',
              label: t('dialogs.event.scope.occurrence'),
              run: () => removeOccurrence(false),
            },
            ...(invitationLocked(opts.calendar, ev)
              ? []
              : [
                  {
                    key: 'thisAndFuture',
                    label: t('dialogs.event.scope.thisAndFuture'),
                    run: () => removeThisAndFuture(false),
                  },
                ]),
            {
              // A whole-series delete can still email a cancellation; the
              // adapters tolerate send-cancellations from a non-organizer (fall
              // back to a plain delete), so `attendees > 0` is safe here.
              key: 'series',
              label: t('dialogs.event.scope.series'),
              destructive: true,
              run: () => deleteWith(ev.attendees.length > 0),
            },
          ],
        });

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

  // The provider cancels for the attendees whatever it is asked (iCloud,
  // Microsoft 365): no notify/silent choice it would not keep, the sentence
  // that says so instead, and every scope notifies (decision 80a).
  const alwaysOccurrenceDialog = (sentence: string) =>
    showEventScopeDialog({
      title: t('dialogs.deleteScope.title'),
      message: `${t('dialogs.deleteScope.message', { title: ev.title })} ${sentence}`,
      cancelLabel: t('dialogs.deleteScope.cancel'),
      options: [
        {
          key: 'occurrence',
          label: t('dialogs.deleteScope.occurrence'),
          destructive: true,
          run: () => removeOccurrence(true),
        },
        {
          key: 'thisAndFuture',
          label: t('dialogs.deleteScope.thisAndFuture'),
          destructive: true,
          run: () => removeThisAndFuture(true),
        },
        {
          key: 'series',
          label: t('dialogs.deleteScope.series'),
          destructive: true,
          run: () => deleteWith(true),
        },
      ],
    });

  const alwaysAlert = (sentence: string) =>
    Alert.alert(
      t('dialogs.event.cancelChoice.title'),
      t('dialogs.event.cancelChoice.alwaysMessage', { title: ev.title, sentence }),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.event.cancelChoice.cancelMeeting'),
          style: 'destructive',
          onPress: () => deleteWith(true),
        },
      ],
    );

  /**
   * What the delete says about who is told: the organizer gets a decline
   * (83b), nobody hears of it at all (`scheduling_silenced`, 98), or the
   * server informs the attendees (76a/80a). One chooser, so the four delete
   * paths on this surface cannot drift apart.
   */
  const deleteSentence = (): string => {
    if (invitationLocked(opts.calendar, ev)) return t(declineSentence(ev).key);
    const spec =
      ev.scheduling_silenced === true
        ? silentSentence(opts.calendar, 'cancellation')
        : notifierSentence(opts.calendar, 'cancellation');
    return t(spec.key, spec.values);
  };

  const plainAlert = () => {
    // Removing the account's copy of someone else's meeting tells the
    // organizer it is declined (83b), so the dialog says so and the button
    // names it.
    const declines = invitationLocked(opts.calendar, ev);
    const silent = ev.scheduling_silenced === true;
    Alert.alert(
      t('dialogs.confirm.deleteEventTitle'),
      `${t('dialogs.confirm.deleteEventMessage', { title: ev.title })}${
        declines || silent ? ` ${deleteSentence()}` : ''
      }`,
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: declines ? t('dialogs.event.deleteAndDecline') : t('dialogs.event.delete'),
          style: 'destructive',
          onPress: () => deleteWith(false),
        },
      ],
    );
  };

  // Only a meeting we ORGANIZE on a scheduling provider asks about its
  // attendees; the adapter's reading (`organized_elsewhere`) says whether we
  // do (decision 70a).
  const notice = cancellationNotice(opts.calendar, ev);
  // `'silent'` is `'always'`'s twin: the same one-button shape, because the
  // provider decides either way — only the sentence differs (decision 98).
  if (notice === 'always' || notice === 'silent') {
    const sentence = deleteSentence();
    if (occurrence != null) {
      alwaysOccurrenceDialog(sentence);
      return;
    }
    alwaysAlert(sentence);
    return;
  }
  if (occurrence != null) {
    occurrenceAlert(notice === 'offer');
    return;
  }
  if (notice === 'offer') choiceAlert();
  else plainAlert();
}
