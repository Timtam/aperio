// Shared "move an item to another container" primitives, used by both the
// Move/Copy dialog and the drag-and-drop handlers so the two paths stay in
// lock-step. Pure async functions over the Tauri commands — no React.

import { invoke } from '@tauri-apps/api/core';
import { differenceInCalendarDays } from 'date-fns';

import {
  addEventExdate,
  createEvent as apiCreateEvent,
  updateEvent as apiUpdateEvent,
} from '../api/client';
import type { CalendarEvent, Task } from '../api/types';
import {
  isExpandedOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../intl/recurrence';

// ── Drag-and-drop payloads ──────────────────────────────────────────────
//
// Items carry a JSON payload under a custom MIME type so a drop target in a
// different component (e.g. the sidebar) can resolve the dragged item
// without a shared store. The type is readable on `dragover` (via
// `dataTransfer.types`) to decide whether a row accepts the drop; the
// JSON body is only readable on `drop`.

export const TASK_DND_TYPE = 'application/aperio-task';
export const EVENT_DND_TYPE = 'application/aperio-event';

export interface TaskDragPayload {
  task: Task;
  /** Direct children, moved along with the parent on a cross-list drop. */
  children: Task[];
}

/** Arm a task drag. Also sets the legacy `text/aperio-task` id used by the
 *  week planner's day-drop (schedule-by-drop). */
export function setTaskDrag(
  dt: DataTransfer,
  task: Task,
  children: Task[],
): void {
  dt.setData(TASK_DND_TYPE, JSON.stringify({ task, children }));
  dt.setData('text/aperio-task', task.id);
  dt.effectAllowed = 'move';
}

export function readTaskDrag(dt: DataTransfer): TaskDragPayload | null {
  const raw = dt.getData(TASK_DND_TYPE);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as TaskDragPayload;
  } catch {
    return null;
  }
}

export function setEventDrag(dt: DataTransfer, event: CalendarEvent): void {
  dt.setData(EVENT_DND_TYPE, JSON.stringify(event));
  dt.effectAllowed = 'move';
}

export function readEventDrag(dt: DataTransfer): CalendarEvent | null {
  const raw = dt.getData(EVENT_DND_TYPE);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as CalendarEvent;
  } catch {
    return null;
  }
}

/**
 * Move a task into a different **section** within the same list (no list
 * change). Subtasks render under their parent regardless of section, so
 * only the parent's `section_id` changes. `null` ⇒ no section. The backend
 * routes the section change per provider (local / Todoist Sync / Vikunja
 * bucket). Returns the updated task.
 */
export async function moveTaskToSection(
  task: Task,
  sectionId: string | null,
): Promise<Task> {
  return invoke<Task>('update_task', {
    task: { ...task, section_id: sectionId },
  });
}

/**
 * Move a task (and its direct children) to a different **list**. Mirrors
 * the Move/Copy dialog: pass the previous `list_id` as the cross-adapter
 * move hint (local stays a single UPDATE; external adapters reroute as
 * create-on-target + delete-from-source, which changes the parent id), and
 * re-thread each child onto the returned parent id so the family stays
 * connected.
 */
export async function moveTaskToList(
  task: Task,
  targetListId: string,
  children: Task[],
): Promise<Task> {
  const movedParent = await invoke<Task>('update_task', {
    task: { ...task, list_id: targetListId },
    previousListId: task.list_id,
  });
  for (const child of children) {
    await invoke<Task>('update_task', {
      task: { ...child, list_id: targetListId, parent_id: movedParent.id },
      previousListId: child.list_id,
    });
  }
  return movedParent;
}

/**
 * Schedule a task on a specific day (sets `scheduled_date`, keeps the
 * time-of-day). Used by the week/month planner day-drop. Bumps
 * `updated_at` so sync engines pick up the change.
 */
export async function scheduleTaskOnDay(
  task: Task,
  dayKey: string,
): Promise<Task> {
  return invoke<Task>('update_task', {
    task: {
      ...task,
      scheduled_date: dayKey,
      updated_at: new Date().toISOString(),
    },
  });
}

/**
 * Schedule a task on a specific day AND wall-clock minute (drag onto the
 * hour grid in the day/week grid views). Sets `scheduled_date` plus
 * `scheduled_time` in the editor's `HH:MM:00` wire shape, so the task
 * turns into a timed chip positioned at the drop time.
 */
export async function scheduleTaskAtTime(
  task: Task,
  dayKey: string,
  minuteOfDay: number,
): Promise<Task> {
  const hh = String(Math.floor(minuteOfDay / 60)).padStart(2, '0');
  const mm = String(minuteOfDay % 60).padStart(2, '0');
  return invoke<Task>('update_task', {
    task: {
      ...task,
      scheduled_date: dayKey,
      scheduled_time: `${hh}:${mm}:00`,
      updated_at: new Date().toISOString(),
    },
  });
}

/**
 * Pull a deferred task into the active backlog now by clearing its
 * `resurface_date` (DESIGN §9.12). The "Zukünftig" group's context-menu
 * action — the user decides they want the task back before its scheduled
 * resurface day.
 */
export async function surfaceTaskNow(task: Task): Promise<Task> {
  return invoke<Task>('update_task', {
    task: {
      ...task,
      resurface_date: null,
      updated_at: new Date().toISOString(),
    },
  });
}

/**
 * Send a task back to the **backlog**: clears the scheduled date/time so it's
 * unscheduled, but KEEPS the deadline. A deadline is independent data (when the
 * task is *due*, not when you planned to work on it), so unscheduling must not
 * silently drop it — that was a data-loss bug. A backlog task with a deadline
 * still surfaces on its deadline day in the calendar + the backlog's deadline
 * rail, and the day-start review still flags it as the deadline nears. Mirrors
 * the plan dialog's "back to backlog" — a completed task reopens to `open`.
 */
export async function moveTaskToBacklog(task: Task): Promise<Task> {
  return invoke<Task>('update_task', {
    task: {
      ...task,
      scheduled_date: null,
      scheduled_time: null,
      status: task.status === 'completed' ? 'open' : task.status,
      updated_at: new Date().toISOString(),
    },
  });
}

/**
 * Move an event to a different **calendar**. For a recurring series we move
 * the master (never a single occurrence); the previous `calendar_id` is the
 * cross-adapter move hint, so external adapters reroute as create+delete
 * rather than PUT-to-a-nonexistent-resource.
 */
export async function moveEventToCalendar(
  event: CalendarEvent,
  targetCalendarId: string,
): Promise<void> {
  const seriesId = seriesIdOf(event);
  await apiUpdateEvent(
    { ...event, id: seriesId, calendar_id: targetCalendarId },
    event.calendar_id,
  );
}

export type MoveCopyMode = 'move' | 'copy';
/** For a recurring event: act on just the focused occurrence, or the whole
 *  series. Ignored for non-recurring events. */
export type MoveCopyScope = 'occurrence' | 'series';

/**
 * Move or copy an event to another calendar, honouring the recurrence scope
 * (DESIGN.md §7.5 — "Nur diesen Termin" vs. "Gesamte Serie").
 *
 *  - **series** → move the master row, or copy it with its recurrence rule.
 *  - **occurrence** → create a STANDALONE single event (no recurrence) at the
 *    occurrence's concrete time on the target. For a MOVE we then EXDATE the
 *    source series so the instance is detached — created first, excluded
 *    second, so a failed create never silently drops the occurrence.
 *
 * `occurrence` scope only takes effect for an actual expanded occurrence; a
 * plain master row falls back to whole-series behaviour.
 */
export async function moveOrCopyEvent(
  event: CalendarEvent,
  targetCalendarId: string,
  mode: MoveCopyMode,
  scope: MoveCopyScope = 'series',
): Promise<void> {
  const asOccurrence = scope === 'occurrence' && isExpandedOccurrence(event);

  // Whole-series move is the only path that updates the existing row in place
  // (and lets external adapters reroute via the move hint). Everything else
  // creates a row on the target.
  if (mode === 'move' && !asOccurrence) {
    await moveEventToCalendar(event, targetCalendarId);
    return;
  }

  await apiCreateEvent({
    calendar_id: targetCalendarId,
    title: event.title,
    description: event.description,
    location: event.location,
    start: event.start,
    end: event.end,
    all_day: event.all_day,
    // Occurrence scope detaches into a single event; series keeps the rule.
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

/**
 * Move an event to another calendar DAY (drag-and-drop in the week /
 * month planners). The wall-clock time and the duration stay; only the
 * day shifts (`setDate` keeps the local time across DST transitions).
 *
 * Recurrence scope mirrors §7.5:
 *  - **series** — update the MASTER row with the dragged occurrence's
 *    shifted dates, re-anchoring the whole series on the new day (the
 *    same master-row semantics `moveEventToCalendar` uses; works for
 *    external providers without needing to fetch the master).
 *  - **occurrence** — detach: create a STANDALONE event on the target
 *    day, then EXDATE the source occurrence (created first, excluded
 *    second, so a failed create never loses the occurrence).
 *
 * Returns false for a same-day drop (no-op — matches the task DnD
 * behaviour for the "dragged a few pixels" misfire).
 */
export async function moveEventToDay(
  event: CalendarEvent,
  targetDayKey: string,
  scope: MoveCopyScope = 'series',
): Promise<boolean> {
  const [y, m, d] = targetDayKey.split('-').map(Number);
  if (!y || !m || !d) return false;
  const delta = differenceInCalendarDays(
    new Date(y, m - 1, d),
    new Date(event.start),
  );
  if (delta === 0) return false;
  const shift = (iso: string) => {
    const when = new Date(iso);
    when.setDate(when.getDate() + delta);
    return when.toISOString();
  };

  if (scope === 'occurrence' && isExpandedOccurrence(event)) {
    await apiCreateEvent({
      calendar_id: event.calendar_id,
      title: event.title,
      description: event.description,
      location: event.location,
      start: shift(event.start),
      end: shift(event.end),
      all_day: event.all_day,
      recurrence: null,
      color_label: event.color_label,
      reminders: event.reminders,
      sound: event.sound,
      attendees: event.attendees,
    });
    const occIso = occurrenceIsoOf(event);
    if (occIso) {
      await addEventExdate(seriesIdOf(event), occIso, event.calendar_id);
    }
    return true;
  }

  await apiUpdateEvent({
    ...event,
    id: seriesIdOf(event),
    start: shift(event.start),
    end: shift(event.end),
  });
  return true;
}
