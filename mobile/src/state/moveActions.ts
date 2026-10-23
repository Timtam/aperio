// "Move or copy an item to another container" primitives — the RN port of the
// desktop moveActions, minus the drag-and-drop payloads (mobile has no DnD).
// Pure async functions over the cal-ffi bridge; the MoveCopyModal drives them.

import type { Task } from '@aperio/shared';
import {
  afterLanded,
  createsInSteps,
  isSeriesOccurrence,
  newSeriesRecurrence,
  occurrenceIsoOf,
  organizerOf,
  readSeriesRows,
  seriesIdOf,
  type StepWriteOptions,
  writeWithoutWaiting,
} from '@aperio/shared';

import {
  addEventExdate,
  createEvent,
  getEventById,
  getSeriesRows,
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
 *
 * Returns the rows as the host returned them, the task first, then each
 * child: one whose original could not be deleted carries `original_left_in`
 * (decision 277), for the surface to say.
 */
export async function moveTaskToList(
  task: Task,
  targetListId: string,
  children: Task[],
  /** Where inside the target list it lands, or `null` for no section. The
   *  task's existing `section_id` is not it: that id means something only
   *  inside the list it just left. */
  sectionId: string | null,
): Promise<Task[]> {
  const movedParent = await updateTask(
    { ...task, list_id: targetListId, section_id: sectionId },
    task.list_id,
  );
  const moved = [movedParent];
  try {
    for (const child of children) {
      const movedChild = await updateTask(
        {
          ...child,
          list_id: targetListId,
          parent_id: movedParent.id,
          // A family in two sections is the same surprise as one in two lists.
          section_id: sectionId,
        },
        child.list_id,
      );
      moved.push(movedChild);
    }
  } catch (err) {
    // The task itself moved: a subtask's failure never reads as "nothing
    // was changed" (decision 277).
    throw afterLanded(err, 'subtasks');
  }
  return moved;
}

/**
 * Move or copy a task to another list, and into a section of it. A move
 * re-uses the shared move primitive; a copy creates a fresh parent row on the
 * target, then re-parents each child copy onto the new id — the original
 * family stays put. Copies land in a list whose MEMBERS differ, so they still
 * start unassigned; the section is now asked for rather than assumed.
 *
 * Returns the moved rows (see `moveTaskToList`); a copy deletes nothing and
 * returns none.
 */
export async function moveOrCopyTask(
  task: Task,
  targetListId: string,
  mode: MoveCopyMode,
  children: Task[],
  sectionId: string | null,
): Promise<Task[]> {
  if (mode === 'move') {
    return moveTaskToList(task, targetListId, children, sectionId);
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
    section_id: sectionId,
    color_label: task.color_label,
    reminders: task.reminders,
    assignees: [],
    sound: task.sound,
  });
  // The copy of the task itself stands: a subtask's failure never reads as
  // "nothing was changed" (decision 277).
  try {
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
        section_id: sectionId,
        color_label: child.color_label,
        reminders: child.reminders,
        assignees: [],
        sound: child.sound,
      });
    }
  } catch (err) {
    throw afterLanded(err, 'subtasks');
  }
  return [];
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
 *
 * The whole series is moved or copied as the series: a row of a series (an
 * expanded occurrence, or a provider override) starts at that occurrence and
 * an override carries no rule, so the master is loaded. Written from the row,
 * the series started at that occurrence and could lose its rule.
 *
 * Returns the moved row of a whole-series move, the one write that deletes an
 * original: where it could not, the row carries `original_left_in` (decision
 * 277), for the surface to say. `null` for everything else.
 */
export async function moveOrCopyEvent(
  event: CalendarEvent,
  targetCalendarId: string,
  mode: MoveCopyMode,
  scope: MoveCopyScope = 'series',
  steps: StepWriteOptions = {},
): Promise<CalendarEvent | null> {
  const asOccurrence = scope === 'occurrence' && isSeriesOccurrence(event);
  // The other calendar creates the series anew: in steps where it writes its
  // deletions so (decisions 227, 265). Mirrors the desktop.
  const wait = steps.wait ?? writeWithoutWaiting;

  if (mode === 'move' && !asOccurrence) {
    const series = await newSeriesRowOf(event);
    return wait(createsInSteps(steps.target, series.recurrence), () =>
      updateEvent({ ...series, calendar_id: targetCalendarId }, series.calendar_id),
    );
  }

  const source = asOccurrence ? event : await newSeriesRowOf(event);
  await wait(!asOccurrence && createsInSteps(steps.target, source.recurrence), () =>
    createEvent(
      {
        calendar_id: targetCalendarId,
        title: source.title,
        description: source.description,
        location: source.location,
        start: source.start,
        end: source.end,
        all_day: source.all_day,
        recurrence: asOccurrence ? null : source.recurrence,
        color_label: source.color_label,
        reminders: source.reminders,
        sound: source.sound,
        attendees: source.attendees,
        ...organizerOf(source),
      },
      // A copy keeps the series' zone, or none, as the source has it
      // (decision 262): its deletions are instants on that clock.
      { preserveRecurrenceZone: true },
    ),
  );

  if (mode === 'move' && asOccurrence) {
    const occIso = occurrenceIsoOf(event);
    if (occIso) {
      await addEventExdate(seriesIdOf(event), occIso, event.calendar_id);
    }
  }
  return null;
}

/**
 * The whole series `event` belongs to, as the NEW series a move or a copy
 * writes (decisions 224, 259): the master (`seriesRowOf`), its rows read by its
 * id, and as exceptions exactly its real deletions (`newSeriesRecurrence`). An
 * occurrence changed elsewhere is no deletion: the new series shows it at its
 * usual time. A single event is itself. A read that fails is a series that
 * could not be loaded, and nothing is written. Mirrors the desktop.
 */
async function newSeriesRowOf(event: CalendarEvent): Promise<CalendarEvent> {
  const series = await seriesRowOf(event);
  if (!series.recurrence?.rrule) return series;
  let rows: CalendarEvent[];
  try {
    rows = (await readSeriesRows(series, series.start, getSeriesRows)).rows;
  } catch {
    throw new SeriesNotLoadedError(series.id);
  }
  return { ...series, recurrence: newSeriesRecurrence(series, rows) };
}

/** The row that stands for the whole series `event` belongs to: `event` itself
 *  when it is that row, else the loaded master. Throws when the master cannot
 *  be loaded, so nothing is written from an occurrence's fields. */
async function seriesRowOf(event: CalendarEvent): Promise<CalendarEvent> {
  const seriesId = seriesIdOf(event);
  if (event.id === seriesId) return event;
  const master = await getEventById(seriesId, event.calendar_id);
  if (!master) {
    throw new SeriesNotLoadedError(seriesId);
  }
  return master;
}

/** The series a row belongs to could not be loaded, so nothing was written:
 *  an occurrence's own fields cannot stand in for the series. The surface names
 *  the event in the user's language. */
export class SeriesNotLoadedError extends Error {
  readonly seriesId: string;

  constructor(seriesId: string) {
    super(`the series ${seriesId} could not be loaded; nothing was changed`);
    this.name = 'SeriesNotLoadedError';
    this.seriesId = seriesId;
  }
}
