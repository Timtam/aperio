import type { CalendarEvent } from '../api/types';
import { deleteEventById, getEventById, getSeriesRows, updateEvent } from '../api/client';
import {
  planSeriesSplit,
  readSeriesRows,
  seriesIdOf,
  type SeriesDeleteOutcome,
} from '../intl/recurrence';

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
 * When nothing the calendar shows comes before the cutoff — the first
 * occurrence, or every earlier one deleted, counting the occurrences the
 * provider keeps as rows of their own (decision 125) — there is nothing to
 * keep, and the series is DELETED (decision 118). Truncating it wrote a rule that ends before
 * it starts: the views showed nothing, but the reminders fell back to the
 * series start and went on ringing, on every device. The delete also retires
 * whatever Aperio keeps under the series' id — private reminders, the group
 * membership — as any delete does. Which of the two happened is returned, so
 * the caller can say it.
 *
 * If the master genuinely carries no recurrence, this degrades to a plain delete
 * of that single event. But a master we could NOT load (null) is a HARD ERROR,
 * never a fall-through to a whole-series delete: conflating "couldn't fetch" with
 * "no recurrence" would silently wipe the earlier occurrences the user meant to
 * keep (and email a full cancellation), which is the exact opposite of the
 * intent. The caller surfaces the thrown message. So is a cutoff that cannot be
 * read: it would be written as the series' end.
 *
 * A cross-client single-occurrence modification synced in as a SEPARATE
 * RECURRENCE-ID override (CalDAV/iCloud + Google) would otherwise survive the
 * truncation as a ghost; the `truncate_tail_overrides` flag on the update asks
 * the adapter to drop the overrides in the dropped tail (CalDAV re-writes the
 * resource without them; Google cancels the tail instance events). EWS keeps
 * modified occurrences inline, so its truncation drops them for free.
 */
export async function deleteThisAndFuture(
  ev: CalendarEvent,
  occurrenceIso: string,
  sendCancellations: boolean,
): Promise<SeriesDeleteOutcome> {
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
    return 'deleted';
  }
  // With the rows the provider keeps for single occurrences: an occurrence
  // changed in Outlook is listed among the master's exceptions, and only its
  // own row says it is still there (decision 125).
  const { rows } = await readSeriesRows(master, occurrenceIso, getSeriesRows);
  const plan = planSeriesSplit(master, occurrenceIso, rows);
  if (plan == null) {
    throw new Error(
      `Could not read where to cut the recurring series "${ev.title}"; ` +
        'no changes were made.',
    );
  }
  if (plan.kind === 'whole') {
    await deleteEventById(seriesId, ev.calendar_id, sendCancellations);
    return 'deleted';
  }
  await updateEvent({
    ...master,
    recurrence: { ...master.recurrence, rrule: plan.headRule },
    send_invitations: sendCancellations,
    // Ask the adapter to drop any provider-side override in the dropped tail
    // (CalDAV/iCloud + Google) so it doesn't survive as a ghost occurrence.
    truncate_tail_overrides: true,
  });
  return 'truncated';
}
