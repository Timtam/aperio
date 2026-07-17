import type { CalendarEvent } from '../api/types';
import { deleteEventById, getEventById, updateEvent } from '../api/client';
import { seriesIdOf, truncateRRuleBefore } from '../intl/recurrence';

/**
 * "Delete this and all following occurrences": truncate the master series so it
 * ends just before `occurrenceIso`, keeping the earlier occurrences.
 *
 * The expanded occurrence row carries the master's recurrence but NOT the
 * master's own start/fields (its `start` is the occurrence's), so we fetch the
 * master, set its recurrence `UNTIL` one second before the cutoff, and write it
 * back through the normal update path — no new backend surface. Falls back to a
 * plain delete when there's nothing to truncate (no series / no occurrence).
 * `sendCancellations` asks the provider to notify attendees of the change.
 */
export async function deleteThisAndFuture(
  ev: CalendarEvent,
  occurrenceIso: string,
  sendCancellations: boolean,
): Promise<void> {
  const seriesId = seriesIdOf(ev);
  const master = await getEventById(seriesId);
  if (!master?.recurrence?.rrule) {
    await deleteEventById(seriesId, ev.calendar_id, sendCancellations);
    return;
  }
  const rrule = truncateRRuleBefore(
    master.recurrence.rrule,
    new Date(occurrenceIso),
  );
  await updateEvent({
    ...master,
    recurrence: { ...master.recurrence, rrule },
    send_invitations: sendCancellations,
  });
}
