import {
  eventTapParams,
  isBirthdayEventId,
  reminderTapOf,
  reminderTapOfUrl,
  type ReminderTap,
} from '@aperio/shared';
import * as Linking from 'expo-linking';
import * as Notifications from 'expo-notifications';
import { useEffect, useRef, useState } from 'react';

import { getEventById, listCalendars } from '../api/calendar';
import { logLine } from '../api/logs';
import { navigateNested, whenNavigationReady } from '../navigation/navigationRef';
import { useAppLockLocked } from '../state/appLockContext';
import { closeDayStartReview, requestDayStartReview } from '../state/dayStartRequest';

let tapCount = 0;

/** Whether the address the app was launched with was taken: once per run of
 *  the JavaScript, as `Linking.getInitialURL` answers with it for the whole
 *  run (as in IncomingFileImport). */
let initialUrlTaken = false;

/** A route of its own for every tap: navigating to an editor of the same name
 *  otherwise reuses the one that is up and replaces its params under state
 *  seeded from the old ones — unsaved edits gone, the old scope kept. */
function newTapToken(): string {
  tapCount += 1;
  return `tap:${Date.now()}:${tapCount}`;
}

/**
 * Open what a tapped notification is about. `'locked'` when the lock came up
 * while the event was being read: nothing was opened, and the tap waits for
 * the unlock again.
 */
async function openTap(tap: ReminderTap, isLocked: () => boolean): Promise<'opened' | 'locked'> {
  await whenNavigationReady();
  if (tap.kind === 'dayStart') {
    requestDayStartReview();
    return 'opened';
  }
  let open: () => void;
  if (tap.kind === 'task') {
    open = () =>
      navigateNested('TasksTab', 'TaskEditor', {
        taskId: tap.taskId,
        listId: tap.listId,
        tapToken: newTapToken(),
      });
  } else {
    // The row decides how the occurrence opens: a series, a single change of
    // one, someone else's meeting. Read from the cache, so it costs no
    // network; a failed read opens the event as it is. The calendars FIRST:
    // an external event resolves through the route their listing fills, and
    // on a cold start nothing else has filled it yet.
    const calendars = await listCalendars().catch(() => []);
    const row = isBirthdayEventId(tap.eventId)
      ? null
      : await getEventById(tap.eventId, tap.calendarId).catch(() => null);
    const calendar = calendars.find((c) => c.id === tap.calendarId) ?? null;
    const params = eventTapParams(tap, row, calendar);
    open = () =>
      navigateNested('CalendarTab', 'EventEditor', { ...params, tapToken: newTapToken() });
  }
  // The reads take a moment, and the app can be covered again by then.
  if (isLocked()) return 'locked';
  // The day-start review is a native modal: an editor opened under it would
  // be out of sight and out of reach.
  closeDayStartReview();
  open();
  return 'opened';
}

/**
 * A tap on a reminder opens its appointment or task; a tap on the day-start
 * notification opens the day-start review (TODO A5, DESIGN §14.3); a tap on a
 * Live Activity opens its appointment (DESIGN-live-activities.md).
 *
 * The response is the LAST one the OS handed over, which also covers the tap
 * that started the app. It is cleared once taken, so a remount does not open
 * it again, and acted on only while the app lock is open — the editor and the
 * review present above everything, the lock cover included.
 */
export function ReminderTapRouter() {
  const response = Notifications.useLastNotificationResponse();
  const locked = useAppLockLocked();
  const lockedRef = useRef(locked);
  lockedRef.current = locked;
  const handled = useRef<string | null>(null);
  const [waiting, setWaiting] = useState<ReminderTap | null>(null);

  useEffect(() => {
    if (response == null) return;
    // A tap on the notification itself; an action button would mean something
    // else (TODO A5: snooze, done).
    if (response.actionIdentifier !== Notifications.DEFAULT_ACTION_IDENTIFIER) return;
    const seen = `${response.notification.request.identifier}@${response.notification.date}`;
    if (handled.current === seen) return;
    handled.current = seen;
    Notifications.clearLastNotificationResponse();
    const tap = reminderTapOf(response.notification.request.content.data);
    if (tap != null) setWaiting(tap);
  }, [response]);

  // A tapped Live Activity opens Aperio with its address, at launch or while
  // it runs. Other addresses (a shared calendar file) are someone else's.
  useEffect(() => {
    const take = (url: string | null) => {
      const tap = url ? reminderTapOfUrl(url) : null;
      if (tap != null) setWaiting(tap);
    };
    if (!initialUrlTaken) {
      initialUrlTaken = true;
      void Linking.getInitialURL().then(take);
    }
    const subscription = Linking.addEventListener('url', ({ url }) => take(url));
    return () => subscription.remove();
  }, []);

  useEffect(() => {
    if (waiting == null || locked) return;
    const tap = waiting;
    setWaiting(null);
    openTap(tap, () => lockedRef.current)
      .then((outcome) => {
        // Covered again while reading: wait for the next unlock. A newer tap
        // that came in meanwhile wins.
        if (outcome === 'locked') setWaiting((newer) => newer ?? tap);
      })
      .catch((err: unknown) => {
        void logLine(
          'warn',
          `opening a tapped ${tap.kind} notification failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      });
  }, [waiting, locked]);

  return null;
}
