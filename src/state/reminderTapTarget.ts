import {
  eventTapParams,
  isBirthdayEventId,
  type NoticeCalendar,
  type ReminderTap,
} from '@aperio/shared';

import type { EventRangeRequest } from '../api/client';
import type { CalendarEvent, Task } from '../api/types';
import { occurrenceOfSeries } from '../intl/recurrence';
import type { EventEditScope } from './DialogState';

/** What a reminder opens on the desktop. */
export type TapTarget =
  | {
      kind: 'event';
      event: CalendarEvent;
      initialScope?: EventEditScope;
      /** Someone else's meeting: the series, showing this occurrence. */
      shownOccurrence?: string;
    }
  | { kind: 'task'; task: Task }
  | { kind: 'dayStart' }
  /** The item is gone, or could not be read: say so, never open a create
   *  editor in its place. */
  | { kind: 'notFound' };

/** The reads a tap needs, handed in. */
export interface TapReads {
  /** The calendars as listed: whether a meeting is someone else's depends on
   *  its calendar, and listing them is what routes an external event. */
  calendars: readonly (NoticeCalendar & { id: string })[];
  getEventById: (id: string, calendarId?: string) => Promise<CalendarEvent | null>;
  getEvents: (request: EventRangeRequest) => Promise<CalendarEvent[]>;
  getTaskById: (id: string) => Promise<Task | null>;
  getTasks: (listId: string) => Promise<Task[]>;
}

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Resolve a reminder's tap to what the desktop opens (TODO A5, DESIGN §14.3):
 * a click on its toast and Enter in the reminders overview both come here,
 * so both open the same thing.
 *
 * The rule is the phone's, `eventTapParams`: an occurrence opens as that
 * occurrence with the scope as a field of the editor, a single change by its
 * own id, someone else's meeting as the series showing the occurrence's
 * times, a birthday as its read-only summary. This only reads the rows the
 * rule names.
 */
export async function resolveReminderTap(tap: ReminderTap, reads: TapReads): Promise<TapTarget> {
  if (tap.kind === 'dayStart') return { kind: 'dayStart' };
  if (tap.kind === 'task') {
    const task = await taskOf(tap.taskId, tap.listId, reads);
    return task == null ? { kind: 'notFound' } : { kind: 'task', task };
  }
  const calendar = reads.calendars.find((c) => c.id === tap.calendarId) ?? null;
  if (isBirthdayEventId(tap.eventId)) {
    // A birthday has no row to read by id: it is made up when its calendar is
    // listed, one per contact and year, with the age in it.
    const at = new Date(tap.start).getTime();
    const rows = await reads
      .getEvents({
        calendar_id: tap.calendarId,
        start: new Date(at - DAY_MS).toISOString(),
        end: new Date(at + 2 * DAY_MS).toISOString(),
      })
      .catch(() => [] as CalendarEvent[]);
    const row = rows.find((r) => r.id === tap.eventId);
    return row == null ? { kind: 'notFound' } : { kind: 'event', event: row };
  }
  const row = await reads.getEventById(tap.eventId, tap.calendarId).catch(() => null);
  if (row == null) return { kind: 'notFound' };
  const params = eventTapParams(tap, row, calendar);
  // A single change of a series, opened by its own id: it IS the occurrence.
  if (params.initialScope === 'occurrence') {
    return { kind: 'event', event: row, initialScope: 'occurrence' };
  }
  if (params.shownOccurrence != null) {
    return {
      kind: 'event',
      event: row,
      initialScope: 'series',
      shownOccurrence: params.shownOccurrence,
    };
  }
  // An occurrence of an own series: the editor offers the scope itself.
  if (params.occurrence != null) {
    return { kind: 'event', event: occurrenceOfSeries(row, params.occurrence) };
  }
  return { kind: 'event', event: row };
}

/** A task by id. The by-id read knows the local store only, so a task on an
 *  external list is found in its list. */
async function taskOf(taskId: string, listId: string, reads: TapReads): Promise<Task | null> {
  const stored = await reads.getTaskById(taskId).catch(() => null);
  if (stored != null) return stored;
  const tasks = await reads.getTasks(listId).catch(() => [] as Task[]);
  return tasks.find((t) => t.id === taskId) ?? null;
}
