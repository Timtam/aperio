/**
 * What a tap on a reminder opens (TODO A5, DESIGN §14.3 "Öffnen").
 *
 * Both ends of the notification live here: what the scheduler writes into a
 * notification's `data`, and what a tap reads back out of it. A field added on
 * one side and forgotten on the other would mean a tap that opens nothing —
 * silently, and only on a device.
 *
 * A tap on the reminder of an appointment opens THAT occurrence straight away
 * (Toni, 2026-11-07): after a reminder one wants to look — time, place, the
 * join link — not to choose between "this one" and "the whole series". The
 * editor still offers the scope, as a field, for whoever goes on to change
 * something.
 */

import { isBirthdayEventId } from './birthdays';
import { invitationLocked, type NoticeCalendar, type NoticeEvent } from './notifyAttendees';
import { overrideRecurrenceIso } from './recurrence';

/** What the scheduler knows about a reminder when it plans its notification. */
export interface TapSource {
  item_id: string;
  item_kind: 'event' | 'task';
  /** The calendar of an event, the list of a task. */
  container_id: string;
  /** The occurrence's start, as the reminder scan gives it. */
  start: string;
  title: string;
}

/** A notification's `data`: strings and booleans only, as both platforms
 *  carry them unchanged. */
export type TapData = Record<string, string | boolean>;

/** The `data` of an item's reminder notification. */
export function reminderTapData(source: TapSource): TapData {
  return {
    itemId: source.item_id,
    itemKind: source.item_kind,
    containerId: source.container_id,
    start: source.start,
    title: source.title,
  };
}

/** The `data` of the day-start notification. */
export const DAY_START_TAP_DATA: TapData = { dayStart: true };

/** What a tap asks for. */
export type ReminderTap =
  | {
      kind: 'event';
      eventId: string;
      calendarId: string;
      /** The occurrence's start. */
      start: string;
      title: string;
    }
  | { kind: 'task'; taskId: string; listId: string }
  | { kind: 'dayStart' };

/**
 * Read a tapped notification's `data`. `null` for anything else — a
 * notification of an older build (it carried no calendar, so there is nothing
 * to open; the next planning run replaces it), or one that is not Aperio's.
 */
export function reminderTapOf(data: unknown): ReminderTap | null {
  if (data == null || typeof data !== 'object') return null;
  const d = data as Record<string, unknown>;
  if (d.dayStart === true) return { kind: 'dayStart' };
  const text = (key: string) => (typeof d[key] === 'string' && d[key] !== '' ? (d[key] as string) : null);
  const itemId = text('itemId');
  const containerId = text('containerId');
  if (itemId == null || containerId == null) return null;
  if (d.itemKind === 'task') return { kind: 'task', taskId: itemId, listId: containerId };
  if (d.itemKind !== 'event') return null;
  const start = text('start');
  if (start == null || Number.isNaN(Date.parse(start))) return null;
  return {
    kind: 'event',
    eventId: itemId,
    calendarId: containerId,
    start,
    title: text('title') ?? '',
  };
}

/** The event editor's parameters, as far as a tap sets them. */
export interface EventTapParams {
  eventId: string;
  calendarId: string;
  occurrence?: string;
  initialScope?: 'occurrence' | 'series';
  initialTitle?: string;
}

/** The row behind the tapped reminder, as the cache holds it. */
export interface TapRow extends NoticeEvent {
  /** Present on a series; a single change of one carries none. */
  recurrence?: unknown;
}

/**
 * What to open for an event reminder.
 *
 * `row` is the event the reminder belongs to, read from the cache — the series
 * for an occurrence of one, the row itself for a single change. `null` when it
 * could not be read: the editor then opens the event as it is, which is never
 * wrong, only less precise for a series.
 */
export function eventTapParams(
  tap: Extract<ReminderTap, { kind: 'event' }>,
  row: TapRow | null,
  calendar: NoticeCalendar | null,
): EventTapParams {
  const base = { eventId: tap.eventId, calendarId: tap.calendarId };
  // A synthetic birthday has no row to read its name from.
  if (isBirthdayEventId(tap.eventId)) return { ...base, initialTitle: tap.title };
  // A single change of a series IS the occurrence: it opens by its own id and
  // saves in place, as from the calendar. Its occurrence is the slot it stands
  // in for, which its id names.
  const slot = overrideRecurrenceIso({
    id: tap.eventId,
    start: tap.start,
    end: tap.start,
    recurrence: null,
  });
  if (slot != null) return { ...base, occurrence: slot, initialScope: 'occurrence' };
  if (row?.recurrence == null) return base;
  // Someone else's meeting is read-only apart from the answer and the
  // reminders: there is nothing to scope, so it opens as the series, as from
  // the calendar (77a).
  if (invitationLocked(calendar, row)) return { ...base, initialScope: 'series' };
  // One spelling of the instant for every caller: the one the calendar's own
  // expansion writes.
  return { ...base, occurrence: new Date(tap.start).toISOString() };
}
