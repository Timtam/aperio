import {
  cancellationNotice,
  declineSentence,
  invitationLocked,
  notifierSentence,
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
  return {
    notice,
    offersChoice: notice === 'offer',
    alwaysNotifies: notice === 'always',
    declines,
    sentence: declines ? declineSentence() : notifierSentence(calendar, 'cancellation'),
  };
}
