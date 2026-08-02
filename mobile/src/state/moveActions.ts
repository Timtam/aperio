// "Move or copy an item to another container" primitives — the RN port of the
// desktop moveActions, minus the drag-and-drop payloads (mobile has no DnD).
// Pure async functions over the cal-ffi bridge; the MoveCopyModal drives them.

import type { Task } from '@aperio/shared';
import { isSeriesOccurrence, occurrenceIsoOf, seriesIdOf } from '@aperio/shared';

import {
  addEventExdate,
  createEvent,
  updateEvent,
  type CalendarEvent,
} from '../api/calendar';
import { createTask, updateTask } from '../api/client';

export type MoveCopyMode = 'move' | 'copy';
/** For a recurring event: act on just the focused occurrence, or the whole
 *  series. Ignored for non-recurring events. */
export type MoveCopyScope = 'occurrence' | 'series';

/**
 * Pull a deferred task into the active backlog now by clearing its
 * `resurface_date` (DESIGN §9.12) — the mobile twin of the desktop's "Bring to
 * backlog" chip action. The existing update bridge writes `resurface_date`
 * verbatim, so a null clears it (no dedicated bridge needed).
 */
export async function surfaceTaskNow(task: Task): Promise<Task> {
  return updateTask({ ...task, resurface_date: null });
}

/**
 * Move a task (and its direct children) to a different list. Passes the previous
 * `list_id` as the cross-adapter move hint (local stays a single UPDATE;
 * external adapters reroute as create-on-target + delete-from-source, which
 * changes the parent id), then re-threads each child onto the returned parent id
 * so the family stays connected.
 */
export async function moveTaskToList(
  task: Task,
  targetListId: string,
  children: Task[],
): Promise<Task> {
  const movedParent = await updateTask({ ...task, list_id: targetListId }, task.list_id);
  for (const child of children) {
    await updateTask(
      { ...child, list_id: targetListId, parent_id: movedParent.id },
      child.list_id,
    );
  }
  return movedParent;
}

/**
 * Move or copy a task to another list. A move re-uses the shared move primitive;
 * a copy creates a fresh parent row on the target, then re-parents each child
 * copy onto the new id — the original family stays put. Copies land in another
 * list whose sections/members differ, so they start ungrouped + unassigned.
 */
export async function moveOrCopyTask(
  task: Task,
  targetListId: string,
  mode: MoveCopyMode,
  children: Task[],
): Promise<void> {
  if (mode === 'move') {
    await moveTaskToList(task, targetListId, children);
    return;
  }
  const newParent = await createTask({
    list_id: targetListId,
    title: task.title,
    description: task.description,
    status: task.status,
    priority: task.priority,
    effort: task.effort,
    scheduled_date: task.scheduled_date,
    scheduled_time: task.scheduled_time,
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
    deadline_reminder_days: task.deadline_reminder_days,
    recurrence: task.recurrence,
    parent_id: null,
    section_id: null,
    color_label: task.color_label,
    reminders: task.reminders,
    assignees: [],
    sound: task.sound,
  });
  for (const child of children) {
    await createTask({
      list_id: targetListId,
      title: child.title,
      description: child.description,
      status: child.status,
      priority: child.priority,
      effort: child.effort,
      scheduled_date: child.scheduled_date,
      scheduled_time: child.scheduled_time,
      deadline_date: child.deadline_date,
      deadline_time: child.deadline_time,
      deadline_reminder_days: child.deadline_reminder_days,
      recurrence: child.recurrence,
      parent_id: newParent.id,
      section_id: null,
      color_label: child.color_label,
      reminders: child.reminders,
      assignees: [],
      sound: child.sound,
    });
  }
}

/**
 * Move or copy an event to another calendar, honouring the recurrence scope
 * (DESIGN.md §7.5 — "Only this event" vs. "Entire series").
 *
 *  - **series** → move the master row in place (external adapters reroute via
 *    the move hint), or copy it with its recurrence rule.
 *  - **occurrence** → create a STANDALONE single event (no recurrence) at the
 *    occurrence's concrete time on the target. For a MOVE we then EXDATE the
 *    source series so the instance is detached — created first, excluded second,
 *    so a failed create never silently drops the occurrence.
 *
 * Occurrence scope only takes effect for an actual expanded occurrence; a plain
 * master row falls back to whole-series behaviour.
 */
export async function moveOrCopyEvent(
  event: CalendarEvent,
  targetCalendarId: string,
  mode: MoveCopyMode,
  scope: MoveCopyScope = 'series',
): Promise<void> {
  const asOccurrence = scope === 'occurrence' && isSeriesOccurrence(event);

  if (mode === 'move' && !asOccurrence) {
    await updateEvent(
      { ...event, id: seriesIdOf(event), calendar_id: targetCalendarId },
      event.calendar_id,
    );
    return;
  }

  await createEvent({
    calendar_id: targetCalendarId,
    title: event.title,
    description: event.description,
    location: event.location,
    start: event.start,
    end: event.end,
    all_day: event.all_day,
    recurrence: asOccurrence ? null : event.recurrence,
    color_label: event.color_label,
    reminders: event.reminders,
    sound: event.sound,
    attendees: event.attendees,
  });

  if (mode === 'move' && asOccurrence) {
    const occIso = occurrenceIsoOf(event);
    if (occIso) {
      await addEventExdate(seriesIdOf(event), occIso, event.calendar_id);
    }
  }
}
