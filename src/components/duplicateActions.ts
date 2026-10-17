import { eventWriteFailureReason, organizerOf } from '@aperio/shared';

import {
  createEvent as apiCreateEvent,
  createTask as apiCreateTask,
} from '../api/client';
import type { CalendarEvent, Task } from '../api/types';
import { newSeriesRowOf, SeriesNotLoadedError } from '../state/moveActions';

// ────────────────────────────────────────────────────────────────────────────
// Duplicate helpers — used by Ctrl+D, no dialog needed.
// ────────────────────────────────────────────────────────────────────────────

/**
 * Duplicate an appointment into its own calendar. Any row of a series — an
 * expanded occurrence, a provider's row for a changed occurrence, the master —
 * duplicates the WHOLE series from its own start (decision 260), as a copy
 * into the same calendar does: with its real deletions only and its zone as
 * the source has it (`newSeriesRowOf`, decisions 259, 262). Written from the
 * occurrence, it started there and its COUNT ran on past the series' end.
 */
export async function duplicateEvent(event: CalendarEvent): Promise<void> {
  const source = await newSeriesRowOf(event);
  await apiCreateEvent(
    {
      calendar_id: source.calendar_id,
      title: source.title,
      description: source.description,
      location: source.location,
      start: source.start,
      end: source.end,
      all_day: source.all_day,
      recurrence: source.recurrence,
      color_label: source.color_label,
      reminders: source.reminders,
      sound: source.sound,
      attendees: source.attendees,
      ...organizerOf(source),
    },
    { preserveRecurrenceZone: true },
  );
}

/**
 * What a failed duplicate says (decision 261): the series that could not be
 * loaded, as a move says it, or the reason in words.
 */
export function duplicateFailedSentence(
  err: unknown,
  title: string,
  t: (key: string, values?: Record<string, unknown>) => string,
): string {
  if (err instanceof SeriesNotLoadedError) {
    return t('dialogs.event.seriesLoadFailed', { title });
  }
  return t('actions.duplicateFailed', { title, reason: eventWriteFailureReason(err, t) });
}

export async function duplicateTask(task: Task): Promise<void> {
  await apiCreateTask({
    list_id: task.list_id,
    title: task.title,
    description: task.description,
    status: task.status,
    priority: task.priority,
    effort: task.effort,
    scheduled_date: task.scheduled_date,
    scheduled_time: task.scheduled_time,
    scheduled_end_time: task.scheduled_end_time ?? null,
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
    deadline_reminder_days: task.deadline_reminder_days,
    recurrence: task.recurrence,
    parent_id: null,
    // Duplicate stays in the same list, so keep its section grouping.
    section_id: task.section_id,
    color_label: task.color_label,
    reminders: task.reminders,
    // Same list ⇒ the assignees stay valid members; copy for fidelity.
    assignees: task.assignees,
    sound: task.sound,
  });
}
