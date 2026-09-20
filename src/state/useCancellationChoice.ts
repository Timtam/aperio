import {
  cancellationNotice,
  declineSentence,
  invitationLocked,
  notifierSentence,
  silentSentence,
  type AttendeeNotice,
} from '@aperio/shared';

import type { CalendarEvent } from '../api/types';
import { useCalendarStore } from './calendarStoreContext';

/**
 * What removing `event` asks or says about its attendees (decisions 70a,
 * 80a), by the rule the mobile delete shares (`cancellationNotice`):
 *
 * - `offersChoice`: a meeting the account organizes, with invitees, on a
 *   provider that can delete silently: "Notify attendees" or "Remove without
 *   notifying".
 * - `alwaysNotifies`: the provider cancels it for the attendees whatever the
 *   request says (iCloud, Microsoft 365); the dialogs say so in `sentence`
 *   instead of offering a choice the provider would not keep.
 * - neither: an attendee's copy, a plain event, a local calendar: a plain
 *   delete.
 *
 * Whether the account organizes the event is the adapter's reading
 * (`organized_elsewhere`), not a comparison of addresses here.
 */
export function useCancellationChoice(event: CalendarEvent | null): {
  notice: AttendeeNotice;
  offersChoice: boolean;
  alwaysNotifies: boolean;
  /** Nobody is told at all: the event's own resource keeps the server out of
   *  its scheduling (RFC 6638 `SCHEDULE-AGENT`). The dialog still SAYS so. */
  silent: boolean;
  declines: boolean;
  sentence: { key: string; values: Record<string, string> };
} {
  const { calendars } = useCalendarStore();
  const calendar = calendars.find((c) => c.id === event?.calendar_id);
  const notice = cancellationNotice(calendar, event);
  // Someone else's meeting has no attendees of the account's own to tell, so
  // `cancellationNotice` says nothing about it — but removing the account's
  // copy is an answer to the organizer (decision 83b).
  const declines = invitationLocked(calendar, event);
  // ONE reading of "nobody is told", used for the flag the dialogs gate on and
  // for the sentence they show. Two spellings of it — the notice here and the
  // raw `scheduling_silenced` elsewhere — is how the surfaces came to disagree
  // about when the sentence appears (decision 98).
  const silent = notice === 'silent';
  return {
    notice,
    offersChoice: notice === 'offer',
    alwaysNotifies: notice === 'always',
    silent,
    declines,
    // Someone else's meeting is answered to its ORGANIZER, and whether that
    // answer leaves the server is the event's own business (`declineSentence`);
    // the account's own meeting speaks about its attendees.
    sentence: declines
      ? declineSentence(event)
      : silent
        ? silentSentence(calendar, 'cancellation')
        : notifierSentence(calendar, 'cancellation'),
  };
}
