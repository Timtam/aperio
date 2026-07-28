// Meetings Aperio itself created — the mobile twin of the desktop
// `attach_meeting` / `detach_meeting` / `event_meeting` commands.
//
// Joining works for ANY meeting through the shared conference detection; this
// is about the ones Aperio owns and can therefore take back down.

import CalFfi from '../../modules/cal-ffi';

import type { CalendarEvent } from './calendar';

/** A meeting on the provider side. */
export interface Meeting {
  id: string;
  join_url: string;
  title: string;
  start_time: string | null;
  end_time: string | null;
  password: string | null;
}

/** The record that Aperio created a meeting for a given event.
 *
 *  Host-local: it names the provider-side meeting this device can still delete.
 *  An event carrying someone else's link has no binding, which is why the
 *  editor offers Join but not Remove for it. */
export interface EventMeetingBinding {
  event_id: string;
  account_id: string;
  meeting_id: string;
  join_url: string;
  created_at: string;
}

/** Create a meeting for an event, write its link into the event, and record the
 *  binding — one call, because doing the three separately is how an event ends
 *  up linking a meeting nobody can delete. */
export const attachMeeting = async (request: {
  event_id: string;
  calendar_id: string;
  account_id: string;
}): Promise<{ event: CalendarEvent; meeting: Meeting }> =>
  JSON.parse(await CalFfi.attachMeetingJson(JSON.stringify(request))) as {
    event: CalendarEvent;
    meeting: Meeting;
  };

/** Delete the meeting Aperio created for an event and take its link back out.
 *  `null` when the event had no meeting of ours. */
export const detachMeeting = async (request: {
  event_id: string;
  calendar_id: string;
}): Promise<CalendarEvent | null> =>
  JSON.parse(
    await CalFfi.detachMeetingJson(JSON.stringify(request)),
  ) as CalendarEvent | null;

/** The meeting Aperio created for this event, if any. */
export const eventMeeting = async (
  eventId: string,
): Promise<EventMeetingBinding | null> =>
  JSON.parse(
    await CalFfi.eventMeetingJson(eventId),
  ) as EventMeetingBinding | null;
