// Shared "move an item to another container" primitives, used by both the
// Move/Copy dialog and the drag-and-drop handlers so the two paths stay in
// lock-step. Pure async functions over the Tauri commands — no React.

import { invoke } from '@tauri-apps/api/core';

import { updateEvent as apiUpdateEvent } from '../api/client';
import type { CalendarEvent, Task } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';

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
 * Send a task back to the **backlog**: clears the scheduled date/time and
 * the deadline so it is truly unscheduled (a lingering deadline would keep
 * pulling it into upcoming views). Mirrors the plan dialog's "back to
 * backlog" — a completed task reopens to `open`.
 */
export async function moveTaskToBacklog(task: Task): Promise<Task> {
  return invoke<Task>('update_task', {
    task: {
      ...task,
      scheduled_date: null,
      scheduled_time: null,
      deadline_date: null,
      deadline_time: null,
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
