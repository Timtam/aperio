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

import { scheduleBackgroundPush } from './syncTriggers';

/** RRULE recurrence + UTC EXDATE instants (the cal_core `EventRecurrence`). */
export interface EventRecurrence {
  rrule: string;
  exceptions: string[];
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

/** One event by id; `null` when absent (the Host returns JSON `null`). */
export const getEventById = async (id: string): Promise<CalendarEvent | null> =>
  JSON.parse(await CalFfi.getEventByIdJson(id)) as CalendarEvent | null;

/** Create an event. `request` is the target calendar plus the NewEvent fields
 *  flattened — the desktop create_event payload shape. */
export const createEvent = async (
  request: { calendar_id: string } & NewEvent,
): Promise<CalendarEvent> => {
  const created = JSON.parse(
    await CalFfi.createEventJson(JSON.stringify(request)),
  ) as CalendarEvent;
  scheduleBackgroundPush();
  return created;
};

/** Full-overwrite update; the event's `calendar_id` selects the route. */
export const updateEvent = async (
  event: CalendarEvent,
): Promise<CalendarEvent> => {
  const updated = JSON.parse(
    await CalFfi.updateEventJson(JSON.stringify(event)),
  ) as CalendarEvent;
  scheduleBackgroundPush();
  return updated;
};

export const deleteEvent = async (
  id: string,
  calendarId: string | null = null,
  sendCancellations: boolean | null = null,
): Promise<void> => {
  await CalFfi.deleteEvent(id, calendarId, sendCancellations);
  scheduleBackgroundPush();
};
