import { detectConference } from './conferencing';

/**
 * Hiding a provider-side meeting that already has a calendar entry.
 *
 * A videoconference account contributes a read-only calendar of its own
 * meetings, so a meeting created in the provider's web UI — which has no
 * calendar entry anywhere — still shows up. But most meetings DO have one:
 * Aperio's own event, or the invitation Outlook wrote. Left alone, those appear
 * twice.
 *
 * The filter is the **join URL**, and that choice is the whole point. It is
 * what the provider issued, what the event carries, and what identifies the
 * meeting to everyone involved — an exact key, not a resemblance. Matching on
 * title and time instead would be worse than it looks: Aperio writes the
 * event's own title into the meeting it creates, so title equality carries
 * almost no evidence, and the times drift apart precisely when an event is
 * moved, which is when a user most wants the two seen as one.
 *
 * This runs in the view layer because that is the only place where all of a
 * window's events are in hand. An adapter is asked for one calendar's events
 * and cannot know what the others hold.
 */

/** Whether an event came from a videoconference account's meetings calendar. */
export function isMeetingCalendarEvent(event: {
  calendar_id?: string | null;
}): boolean {
  return (event.calendar_id ?? '').endsWith('::meetings');
}

/** The join URL an event carries, or `null`. */
function joinUrlOf(event: {
  location?: string | null;
  description?: string | null;
}): string | null {
  return (
    detectConference({
      location: event.location,
      description: event.description,
    })?.joinUrl ?? null
  );
}

/**
 * Drop meetings-calendar events whose meeting is already represented by a real
 * calendar event in the same set.
 *
 * Order-independent and stable: real events are never dropped, only the
 * synthesized ones, and only when something else in view already shows that
 * exact meeting.
 */
export function withoutDuplicateMeetings<
  T extends {
    calendar_id?: string | null;
    location?: string | null;
    description?: string | null;
  },
>(events: T[]): T[] {
  // Collect the links carried by REAL events first — a second synthesized
  // event for the same meeting (two accounts on one site, say) must not
  // suppress the first.
  const claimed = new Set<string>();
  for (const event of events) {
    if (isMeetingCalendarEvent(event)) continue;
    const url = joinUrlOf(event);
    if (url) claimed.add(url);
  }
  if (claimed.size === 0) return events;
  return events.filter((event) => {
    if (!isMeetingCalendarEvent(event)) return true;
    const url = joinUrlOf(event);
    return url == null || !claimed.has(url);
  });
}
