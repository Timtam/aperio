import type { CalendarEvent } from '../api/types';
import { deleteEventById, getEventById, updateEvent } from '../api/client';
import { seriesIdOf, truncateRRuleBefore } from '../intl/recurrence';

/**
 * "Delete this and all following occurrences": truncate the master series so it
 * ends just before `occurrenceIso`, keeping the earlier occurrences.
 *
 * The expanded occurrence row carries the master's recurrence but NOT the
 * master's own start/fields (its `start` is the occurrence's), so we fetch the
 * master (passing the owning calendar so an EXTERNAL master resolves via the SWR
 * cache), set its recurrence `UNTIL` one second before the cutoff, and write it
 * back through the normal update path — no new backend surface.
 * `sendCancellations` asks the provider to notify attendees of the change.
 *
 * If the master genuinely carries no recurrence, this degrades to a plain delete
 * of that single event. But a master we could NOT load (null) is a HARD ERROR,
 * never a fall-through to a whole-series delete: conflating "couldn't fetch" with
 * "no recurrence" would silently wipe the earlier occurrences the user meant to
 * keep (and email a full cancellation), which is the exact opposite of the
 * intent. The caller surfaces the thrown message.
 *
 * KNOWN LIMITATION: only the master's own recurrence + EXDATEs are truncated.
 * A cross-client single-occurrence modification synced in as a SEPARATE
 * RECURRENCE-ID override event (CalDAV/iCloud + Google mint these as `::rid::`
 * ids; EWS keeps modified occurrences inline, so it's unaffected) is NOT
 * enumerated, so an override that falls AFTER the cutoff survives this truncation
 * as a ghost. The same applies to the edit-split path (EventDialog). Handling it
 * would need a bounded query of the tail range for override events + a delete/
 * re-point of each — deferred; Aperio's own occurrence edits (standalone event +
 * master EXDATE) are handled and are the common case.
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
    await deleteEventById(seriesId, ev.calendar_id, sendCancellations);
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
