import {
  createsInSteps,
  eventWriteErrorMessage,
  eventWriteFailureReason,
  eventWriteLeftover,
  organizerOf,
  writeWithoutWaiting,
  type StepWriteOptions,
} from '@aperio/shared';

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
 * Answers with what it wrote from, so the sentence names that.
 */
export async function duplicateEvent(
  event: CalendarEvent,
  steps: StepWriteOptions = {},
): Promise<CalendarEvent> {
  const source = await newSeriesRowOf(event);
  // A series whose calendar writes a new series' deletions in steps is
  // waited for (decisions 227, 265).
  await (steps.wait ?? writeWithoutWaiting)(
    createsInSteps(steps.target, source.recurrence),
    () =>
      apiCreateEvent(
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
      ),
  );
  return source;
}

/**
 * What a duplicate says: the appointment written, by its own title — a
 * changed occurrence's row may carry another — and, for a series, that the
 * whole series was duplicated (decision 260).
 */
export function duplicatedSentence(
  source: CalendarEvent,
  t: (key: string, values?: Record<string, unknown>) => string,
): string {
  return t(source.recurrence?.rrule ? 'actions.duplicatedSeries' : 'actions.duplicated', {
    title: source.title,
  });
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
  // A new series that may stand: said so, and not to be done again (210).
  // Any other leftover says its own sentence.
  const leftover = eventWriteLeftover(err);
  if (leftover?.leftover === 'created-may-stand') return t('actions.copyMayStand', { title });
  if (leftover) return eventWriteErrorMessage(err, t);
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

/**
 * Ctrl+D and the context menu's Duplicate on a task: a flat copy in the same
 * list, said either way — a failure in words, never an unhandled rejection or
 * the console alone (decision 273). The list reads again both ways: the copy
 * shows at once, and one that may stand shows on the next read.
 */
export async function duplicateTaskAndSay(
  task: Task,
  {
    announce,
    invalidateData,
    t,
  }: {
    announce: (message: string) => void;
    invalidateData: () => void;
    t: (key: string, values?: Record<string, unknown>) => string;
  },
): Promise<void> {
  try {
    await duplicateTask(task);
    invalidateData();
    announce(t('actions.duplicated', { title: task.title }));
  } catch (err) {
    invalidateData();
    announce(duplicateFailedSentence(err, task.title, t));
  }
}
