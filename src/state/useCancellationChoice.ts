import { useEffect, useState } from 'react';

import type { CalendarEvent } from '../api/types';
import { useCalendarStore } from './calendarStoreContext';
import { resolveCalendarUserEmail } from './currentUserEmail';

/** Lower-case, `mailto:`-stripped form for comparing addresses. */
function normalizeEmail(value: string | null | undefined): string {
  if (!value) return '';
  return value.trim().replace(/^mailto:/i, '').toLowerCase();
}

/**
 * Decide whether removing `event` should offer the "cancel (notify attendees)
 * vs remove silently" choice instead of a plain delete.
 *
 * Cancelling a meeting is an ORGANIZER action that emails the attendees. We
 * offer the choice only when all three hold:
 *   - the event's calendar is on a scheduling-capable provider
 *     (`supports_scheduling` — true for EWS/Graph/Google/iCloud, false for
 *     local), i.e. the adapter can actually send a cancellation;
 *   - the connected account is the event's **organizer**; and
 *   - the meeting has **attendees** to notify.
 *
 * Otherwise (an attendee's copy, a non-meeting event, a local calendar) the
 * caller falls back to a plain delete. "Who am I" comes from
 * `calendarCurrentUserEmail`, the same source EventRsvp uses.
 */
export function useCancellationChoice(event: CalendarEvent | null): {
  offersChoice: boolean;
} {
  const { calendars } = useCalendarStore();
  const [myEmail, setMyEmail] = useState<string | null>(null);

  const calendarId = event?.calendar_id ?? null;
  const hasAttendees = (event?.attendees?.length ?? 0) > 0;
  const supportsScheduling =
    calendars.find((c) => c.id === calendarId)?.supports_scheduling ?? false;
  // Only bother resolving "me" when the cheap, synchronous preconditions hold.
  const worthChecking = !!calendarId && hasAttendees && supportsScheduling;

  useEffect(() => {
    let cancelled = false;
    if (!worthChecking || !calendarId) {
      setMyEmail(null);
      return;
    }
    resolveCalendarUserEmail(calendarId)
      .then((email) => {
        if (!cancelled) setMyEmail(email);
      })
      .catch(() => {
        if (!cancelled) setMyEmail(null);
      });
    return () => {
      cancelled = true;
    };
  }, [worthChecking, calendarId]);

  const me = normalizeEmail(myEmail);
  const isOrganizer = !!me && normalizeEmail(event?.organizer) === me;
  return { offersChoice: worthChecking && isOrganizer };
}
