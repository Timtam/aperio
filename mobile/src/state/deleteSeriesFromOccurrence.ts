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
 * KNOWN LIMITATION (same as desktop): a cross-client single-occurrence change
 * synced in as a SEPARATE RECURRENCE-ID override event (CalDAV/iCloud + Google
 * `::rid::` ids; EWS keeps them inline, unaffected) that falls AFTER the cutoff is
 * not enumerated, so it survives this truncation. Aperio's own occurrence edits
 * (standalone + EXDATE) are handled and are the common case.
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
  });
}
