import {
  eventTapParams,
  isBirthdayEventId,
  seriesHasOccurrence,
  type EventTapParams,
  type NoticeCalendar,
  type RecurringEventLike,
  type ReminderTap,
  type TapRow,
} from '@aperio/shared';

/** The reads the event editor's parameters need, handed in. */
export interface TapParamReads {
  listCalendars: () => Promise<readonly (NoticeCalendar & { id: string })[]>;
  getEventById: (id: string, calendarId: string) => Promise<(TapRow & RecurringEventLike) | null>;
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
 *
 * `null` when the reminder's occurrence of an own series was deleted since it
 * was planned: there is nothing to open, and an editor on it would bring it
 * back when saved. The caller says so.
 */
export async function eventEditorParams(
  tap: Extract<ReminderTap, { kind: 'event' }>,
  reads: TapParamReads,
): Promise<EventTapParams | null> {
  const calendars = await reads.listCalendars().catch(() => []);
  const row = isBirthdayEventId(tap.eventId)
    ? null
    : await reads.getEventById(tap.eventId, tap.calendarId).catch(() => null);
  const calendar = calendars.find((c) => c.id === tap.calendarId) ?? null;
  const params = eventTapParams(tap, row, calendar);
  if (
    row != null &&
    params.occurrence != null &&
    params.initialScope == null &&
    !seriesHasOccurrence(row, params.occurrence)
  ) {
    return null;
  }
  return params;
}
