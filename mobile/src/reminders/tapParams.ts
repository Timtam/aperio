import {
  eventTapParams,
  isBirthdayEventId,
  type EventTapParams,
  type NoticeCalendar,
  type ReminderTap,
  type TapRow,
} from '@aperio/shared';

/** The reads the event editor's parameters need, handed in. */
export interface TapParamReads {
  listCalendars: () => Promise<readonly (NoticeCalendar & { id: string })[]>;
  getEventById: (id: string, calendarId: string) => Promise<TapRow | null>;
}

/**
 * The event editor's parameters for a reminder: from its notification (the
 * tap router) and from a row of the reminders overview alike, so both open
 * THAT occurrence of a series (TODO A5).
 *
 * The row decides how the occurrence opens: a series, a single change of one,
 * someone else's meeting. Read from the cache, so it costs no network; a
 * failed read opens the event as it is. The calendars FIRST: an external
 * event resolves through the route their listing fills, and on a cold start
 * nothing else has filled it yet.
 */
export async function eventEditorParams(
  tap: Extract<ReminderTap, { kind: 'event' }>,
  reads: TapParamReads,
): Promise<EventTapParams> {
  const calendars = await reads.listCalendars().catch(() => []);
  const row = isBirthdayEventId(tap.eventId)
    ? null
    : await reads.getEventById(tap.eventId, tap.calendarId).catch(() => null);
  const calendar = calendars.find((c) => c.id === tap.calendarId) ?? null;
  return eventTapParams(tap, row, calendar);
}
