import {
  createEvent as apiCreateEvent,
  createTask as apiCreateTask,
} from '../api/client';
import type { CalendarEvent, Task } from '../api/types';

// ────────────────────────────────────────────────────────────────────────────
// Duplicate helpers — used by Ctrl+D, no dialog needed.
// ────────────────────────────────────────────────────────────────────────────

export async function duplicateEvent(event: CalendarEvent): Promise<void> {
  await apiCreateEvent({
    calendar_id: event.calendar_id,
    title: event.title,
    description: event.description,
    location: event.location,
    start: event.start,
    end: event.end,
    all_day: event.all_day,
    recurrence: event.recurrence,
    color_label: event.color_label,
    reminders: event.reminders,
    sound: event.sound,
    attendees: event.attendees,
  });
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
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
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
