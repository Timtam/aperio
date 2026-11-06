import { eventTapParams, isBirthdayEventId, reminderTapOf, type ReminderTap } from '@aperio/shared';
import * as Notifications from 'expo-notifications';
import { useEffect, useRef } from 'react';

import { getEventById, listCalendars } from '../api/calendar';
import { logLine } from '../api/logs';
import { navigateNested, whenNavigationReady } from '../navigation/navigationRef';
import { useAppLockLocked } from '../state/appLockContext';
import { requestDayStartReview } from '../state/dayStartRequest';

/** Open what a tapped notification is about. */
async function openTap(tap: ReminderTap): Promise<void> {
  await whenNavigationReady();
  switch (tap.kind) {
    case 'task':
      navigateNested('TasksTab', 'TaskEditor', { taskId: tap.taskId, listId: tap.listId });
      return;
    case 'dayStart':
      requestDayStartReview();
      return;
    case 'event': {
      // The row decides how the occurrence opens: a series, a single change
      // of one, someone else's meeting. Read from the cache, so it costs no
      // network; a failed read opens the event as it is.
      const [row, calendars] = await Promise.all([
        isBirthdayEventId(tap.eventId)
          ? Promise.resolve(null)
          : getEventById(tap.eventId, tap.calendarId).catch(() => null),
        listCalendars().catch(() => []),
      ]);
      const calendar = calendars.find((c) => c.id === tap.calendarId) ?? null;
      navigateNested('CalendarTab', 'EventEditor', eventTapParams(tap, row, calendar));
      return;
    }
  }
}

/**
 * A tap on a reminder opens its appointment or task; a tap on the day-start
 * notification opens the day-start review (TODO A5, DESIGN §14.3).
 *
 * The response is the LAST one the OS handed over, which also covers the tap
 * that started the app. It is acted on only once the app lock is open: the
 * editor and the review present above everything, the lock cover included.
 * After that it is cleared, so a remount does not open it a second time.
 */
export function ReminderTapRouter() {
  const response = Notifications.useLastNotificationResponse();
  const locked = useAppLockLocked();
  const handled = useRef<string | null>(null);

  useEffect(() => {
    if (response == null || locked) return;
    // A tap on the notification itself; an action button would mean something
    // else (TODO A5: snooze, done).
    if (response.actionIdentifier !== Notifications.DEFAULT_ACTION_IDENTIFIER) return;
    const { identifier } = response.notification.request;
    const seen = `${identifier}@${response.notification.date}`;
    if (handled.current === seen) return;
    handled.current = seen;
    Notifications.clearLastNotificationResponse();
    const tap = reminderTapOf(response.notification.request.content.data);
    if (tap == null) return;
    openTap(tap).catch((err: unknown) => {
      void logLine(
        'warn',
        `opening a tapped ${tap.kind} notification failed: ${err instanceof Error ? err.message : String(err)}`,
      );
    });
  }, [response, locked]);

  return null;
}
