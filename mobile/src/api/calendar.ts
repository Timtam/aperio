// Mobile calendar/event api-client — the engine-reuse boundary for the
// calendar surface (the Host: local + statically-embedded external adapters).
// Mirrors the desktop's calendar command shapes; each body is JSON passthrough
// over a `CalFfi.*` Host call. The JSON wire is the cal_core/desktop serde
// shape, so payloads match the desktop's Tauri commands exactly.
//
// The composite types (Calendar/CalendarEvent/NewEvent/EventRecurrence) are
// defined here for now — like Account in ./accounts — reusing the leaf types
// already in @aperio/shared. They hoist to @aperio/shared in a consolidation
// pass (so the desktop shares them too), the same path the task types took.

import CalFfi from '../../modules/cal-ffi';
import type {
  ContainerColor,
  RecurrenceCapabilities,
  Reminder,
  SoundConfig,
} from '@aperio/shared';

import { notifyCalendarChanged } from '../state/calendarMutations';
import { scheduleBackgroundPush } from './syncTriggers';

/** RRULE recurrence + UTC EXDATE instants (the cal_core `EventRecurrence`). */
export interface EventRecurrence {
  rrule: string;
  exceptions: string[];
  /** IANA zone of the master DTSTART, when the source carried one; drives
   *  DST-correct expansion in `@aperio/shared` recurrence.ts. */
  tzid?: string | null;
}

/** RSVP state of an attendee, where the provider reports it (read-only). */
export type AttendeeStatus =
  | 'needs-action'
  | 'accepted'
  | 'declined'
  | 'tentative'
  | 'delegated';

export interface AttendeeResponse {
  email: string;
  name?: string | null;
  status: AttendeeStatus;
}

/** A calendar enriched with its owning `account_id` (the desktop CalendarRow
 *  wire shape the Host produces). */
export interface Calendar {
  id: string;
  name: string;
  color: ContainerColor | null;
  color_label: string | null;
  read_only: boolean;
  default_sound: SoundConfig | null;
  account_id: string;
  /** Absent → full RFC-5545 support (the Host omits it this slice). */
  recurrence_capabilities?: RecurrenceCapabilities;
  supports_scheduling?: boolean;
  supports_event_color?: boolean;
}

/** A persisted calendar event (the desktop `CalendarEvent` wire shape). */
export interface CalendarEvent {
  id: string;
  calendar_id: string;
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  /** Read-only native colour; never sent back. */
  color_hex?: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
  send_invitations?: boolean;
  created_at: string;
  updated_at: string;
  etag: string | null;
  organizer?: string | null;
  attendee_responses?: AttendeeResponse[];
}

/** A new (unsaved) event — the desktop `NewEvent` wire shape. */
export interface NewEvent {
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
  send_invitations?: boolean;
}

export interface CreateCalendarRequest {
  name: string;
  color_label?: string | null;
}

export interface EventRangeRequest {
  calendar_id: string;
  /** RFC-3339 UTC instant. */
  start: string;
  /** RFC-3339 UTC instant. */
  end: string;
}

// ── Calendars ──────────────────────────────────────────────────────────────

/** All calendars (local + external); also primes the Host's route map, so call
 *  it before event operations. */
export const listCalendars = async (): Promise<Calendar[]> =>
  JSON.parse(await CalFfi.listCalendarsJson()) as Calendar[];

export const createCalendar = async (
  request: CreateCalendarRequest,
): Promise<Calendar> => {
  const created = JSON.parse(
    await CalFfi.createCalendarJson(JSON.stringify(request)),
  ) as Calendar;
  scheduleBackgroundPush();
  return created;
};

export const deleteCalendar = async (id: string): Promise<void> => {
  await CalFfi.deleteCalendar(id);
  scheduleBackgroundPush();
};

// ── Events ───────────────────────────────────────────────────────────────────

export const getEvents = async (
  request: EventRangeRequest,
): Promise<CalendarEvent[]> =>
  JSON.parse(await CalFfi.getEventsJson(JSON.stringify(request))) as CalendarEvent[];

/** One event by id; `null` when absent (the Host returns JSON `null`). Pass the
 *  owning `calendarId` so an EXTERNAL event resolves via the SWR cache — the
 *  local store has no row for it, so without the route the editor would open
 *  empty (and a save would create a duplicate). Omit/null for a local event. */
export const getEventById = async (
  id: string,
  calendarId: string | null = null,
): Promise<CalendarEvent | null> =>
  JSON.parse(await CalFfi.getEventByIdJson(id, calendarId)) as CalendarEvent | null;

/** Create an event. `request` is the target calendar plus the NewEvent fields
 *  flattened — the desktop create_event payload shape. */
export const createEvent = async (
  request: { calendar_id: string } & NewEvent,
): Promise<CalendarEvent> => {
  const created = JSON.parse(
    await CalFfi.createEventJson(JSON.stringify(request)),
  ) as CalendarEvent;
  scheduleBackgroundPush();
  notifyCalendarChanged();
  return created;
};

/**
 * Full-overwrite update; the event's `calendar_id` selects the route.
 * `previousCalendarId` is the calendar the editor loaded the event FROM — pass
 * it when the calendar picker may have changed so the bridge can detect a
 * cross-calendar MOVE (create-on-target + best-effort-delete-from-source);
 * without it a move to an external target would PUT to a non-existent resource
 * and fail with 412. Returns the resulting event (a cross-adapter move returns
 * the freshly-created event at the target, with a new id).
 */
export const updateEvent = async (
  event: CalendarEvent,
  previousCalendarId: string | null = null,
): Promise<CalendarEvent> => {
  const updated = JSON.parse(
    await CalFfi.updateEventJson(JSON.stringify(event), previousCalendarId),
  ) as CalendarEvent;
  scheduleBackgroundPush();
  notifyCalendarChanged();
  return updated;
};

export const deleteEvent = async (
  id: string,
  calendarId: string | null = null,
  sendCancellations: boolean | null = null,
): Promise<void> => {
  await CalFfi.deleteEvent(id, calendarId, sendCancellations);
  scheduleBackgroundPush();
  notifyCalendarChanged();
};

/** Exclude ONE occurrence of a recurring event — append `occurrence` (its
 *  RFC-3339 instant) to the series master's EXDATE so the expansion engine skips
 *  it (the "delete / edit this occurrence only" flow). `calendarId` routes
 *  (null → local). A local change syncs (EventUpdated). */
export const addEventExdate = async (
  id: string,
  occurrence: string,
  calendarId: string | null = null,
): Promise<void> => {
  await CalFfi.addEventExdateJson(id, occurrence, calendarId);
  scheduleBackgroundPush();
  notifyCalendarChanged();
};

/** Parse a free-form attendee entry ("Name <email>" or a bare email) into its
 *  name + email via the shared cal-core parser (synchronous). The parser only
 *  splits on the bracket pair — it does NOT validate, so a bare non-email string
 *  comes back whole as `email`; callers do their own email-shape check. */
export const parseAttendee = (entry: string): { name: string | null; email: string } =>
  CalFfi.parseAttendee(entry);

// ── RSVP (§7.3) ──────────────────────────────────────────────────────────────

/** The connected account's email for `calendarId` — the RSVP "who am I", used
 *  to tell an attendee from the organizer. `null` for local/iCal calendars and
 *  any provider that can't report an identity (which hides the RSVP affordance). */
export const calendarCurrentUserEmail = async (
  calendarId: string,
): Promise<string | null> => CalFfi.calendarCurrentUserEmail(calendarId);

/** RSVP to an invitation: set the connected user's participation `status` on the
 *  meeting. `sendResponse` also emails the reply to the organizer on a
 *  scheduling-capable provider. Only valid on external, non-organizer meetings
 *  (local/unroutable reject). The Host invalidates the event cache, so a refetch
 *  reflects the new status. External-only, so no local sync push is triggered. */
export const respondToEvent = async (
  calendarId: string,
  eventId: string,
  status: AttendeeStatus,
  sendResponse: boolean,
): Promise<void> => {
  await CalFfi.respondToEvent(calendarId, eventId, status, sendResponse);
};

/** One attendee's busy blocks within the queried window (the cal_core `FreeBusy`
 *  wire shape). An empty `slots` array means "no known conflicts" (or the
 *  provider couldn't answer). */
export interface FreeBusySlot {
  /** RFC-3339 UTC instant. */
  start: string;
  end: string;
}
export interface FreeBusy {
  email: string;
  slots: FreeBusySlot[];
}

/** Attendee availability for `emails` over `[rangeStart, rangeEnd]` (RFC-3339)
 *  through the account that owns `calendarId`. Best-effort: returns `[]` for a
 *  local calendar or a provider that can't answer (no error), which the UI reads
 *  as "free/unknown". */
export const queryFreeBusy = async (
  calendarId: string,
  emails: string[],
  rangeStart: string,
  rangeEnd: string,
): Promise<FreeBusy[]> =>
  JSON.parse(
    await CalFfi.queryFreeBusyJson(
      JSON.stringify({
        calendar_id: calendarId,
        emails,
        range_start: rangeStart,
        range_end: rangeEnd,
      }),
    ),
  ) as FreeBusy[];
