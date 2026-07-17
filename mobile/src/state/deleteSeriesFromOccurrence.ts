import { seriesIdOf, truncateRRuleBefore } from '@aperio/shared';

import {
  CalendarEvent,
  deleteEvent,
  getEventById,
  updateEvent,
} from '../api/calendar';

/**
 * "Delete this and all following occurrences": truncate the master series so it
 * ends just before `occurrenceIso`, keeping the earlier occurrences. Mirrors the
 * desktop `deleteThisAndFuture` — the expanded occurrence row carries the
 * master's recurrence but not its own start, so fetch the master (passing the
 * owning calendar so an EXTERNAL master resolves via the SWR cache), set its
 * recurrence `UNTIL` one second before the cutoff, and write it back through the
 * normal update path. `sendCancellations` asks the provider to notify attendees.
 *
 * A master with genuinely no recurrence degrades to a plain delete, but a master
 * we could NOT load (null) is a HARD ERROR: conflating "couldn't fetch" with "no
 * recurrence" would delete the WHOLE series (wiping the earlier occurrences the
 * user meant to keep, plus emailing a full cancellation) — the opposite of intent.
 *
 * A cross-client single-occurrence change synced in as a SEPARATE RECURRENCE-ID
 * override (CalDAV/iCloud + Google) is dropped by the adapter via the
 * `truncate_tail_overrides` flag on the update, so it doesn't survive as a ghost.
 * EWS keeps modified occurrences inline, so its truncation drops them for free.
 */
export async function deleteThisAndFuture(
  ev: CalendarEvent,
  occurrenceIso: string,
  sendCancellations: boolean,
): Promise<void> {
  const seriesId = seriesIdOf(ev);
  const master = await getEventById(seriesId, ev.calendar_id);
  if (master == null) {
    throw new Error(
      `Could not load the recurring series "${ev.title}" to truncate it; ` +
        'no changes were made.',
    );
  }
  if (!master.recurrence?.rrule) {
    await deleteEvent(seriesId, ev.calendar_id, sendCancellations);
    return;
  }
  const rrule = truncateRRuleBefore(
    master.recurrence.rrule,
    new Date(occurrenceIso),
    { allDay: master.all_day },
  );
  await updateEvent({
    ...master,
    recurrence: { ...master.recurrence, rrule },
    send_invitations: sendCancellations,
    // Ask the adapter to drop any provider-side override in the dropped tail
    // (CalDAV/iCloud + Google) so it doesn't survive as a ghost occurrence.
    truncate_tail_overrides: true,
  });
}
