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
import {
  localizeBirthdayCalendarName,
  type Calendar,
  seriesRowsFromHost,
  type SeriesRows,
  type SeriesRowsRequest,
  withCreatedRecurrenceZone,
} from '@aperio/shared';
import i18n from '../../i18n';
import type {
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

/** A calendar as the Host lists it — generated from
 *  `host_core::wire::CalendarRow`, the same declaration the desktop reads. */
export type { Calendar };

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
  /** Transient "this and all following" signal for the next update: when true on
   *  a truncated recurring MASTER, CalDAV/iCloud + Google adapters drop any
   *  RECURRENCE-ID override in the dropped tail so it doesn't ghost. Not
   *  persisted; only the split's update sets it. */
  truncate_tail_overrides?: boolean;
  created_at: string;
  updated_at: string;
  etag: string | null;
  organizer?: string | null;
  /** Someone other than the connected account organizes this event, or the
   *  provider cannot confirm that the account does (decision 70a). Read-only;
   *  the editor then offers no "notify attendees". */
  organized_elsewhere?: boolean;
  /** Never the organizer's row (decision 67a). */
  attendee_responses?: AttendeeResponse[];
  /** The event's resource says the SERVER must not send its scheduling
   *  messages (RFC 6638 `SCHEDULE-AGENT=CLIENT` or `NONE`). Read-only. The
   *  calendar's `always_notifies_attendees` is a fact about the calendar;
   *  this is the one fact about the event that can contradict it, so a dialog
   *  promises a message only where one is sent (live round 6). */
  scheduling_silenced?: boolean;
  /** The meeting is cancelled. Read-only. Cancelled events never fire
   *  reminders and are hidden when "show cancelled events" is off. */
  cancelled?: boolean;
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
  /** For a create derived from an existing event: its organizer and whether
   *  someone else organizes it (`organizerOf`, decision 72a). Never sent to a
   *  provider. */
  organizer?: string | null;
  organized_elsewhere?: boolean;
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

/** All calendars (local + external), a birthday layer named in the UI
 *  language — the same function the desktop wrapper calls; also primes the
 *  Host's route map, so call it before event operations. */
export const listCalendars = async (): Promise<Calendar[]> =>
  (JSON.parse(await CalFfi.listCalendarsJson()) as Calendar[]).map((cal) =>
    localizeBirthdayCalendarName(cal, (key, vars) => i18n.t(key, vars)),
  );

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

/** The cached rows of one series besides its master, whatever their dates, and
 *  how far the cache reaches (decisions 135 and 139) — the desktop's
 *  `get_series_rows`. It rides on the events read so that a native library
 *  older than the app still answers: that one ignores `series_id` and returns
 *  the events of the request's stretch (`seriesRowsFromHost`). */
export const getSeriesRows = async (
  request: SeriesRowsRequest,
): Promise<SeriesRows<CalendarEvent>> =>
  seriesRowsFromHost(
    JSON.parse(await CalFfi.getEventsJson(JSON.stringify(request))) as
      | SeriesRows<CalendarEvent>
      | CalendarEvent[],
  );

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
 *  flattened — the desktop create_event payload shape. `use_calendar_defaults`
 *  says the caller made no reminder choice at all (an untouched editor, a
 *  quick-add): a calendar whose default reminders are set to "attach" then
 *  writes them into the new appointment. Leave it off when `reminders` IS the
 *  choice — including an emptied list. */
export const createEvent = async (
  request: { calendar_id: string; use_calendar_defaults?: boolean } & NewEvent,
  opts: { preserveRecurrenceZone?: boolean } = {},
): Promise<CalendarEvent> => {
  // Stamp the device's local zone onto a brand-new timed recurring rule so it
  // expands DST-correctly (parity with zoned CalDAV series); no-op otherwise.
  //
  // `preserveRecurrenceZone` skips that defaulting: a series-SPLIT tail ("edit
  // this and all following") is a CONTINUATION, not a fresh series, so it must
  // keep the master's zone VERBATIM — including a floating (null) zone. The
  // truncated head (updateEvent, never stamped) and this tail then expand
  // identically, so untouched occurrences never drift an hour across a DST
  // boundary; stamping only the tail would diverge the two halves.
  const created = JSON.parse(
    await CalFfi.createEventJson(
      JSON.stringify({
        ...request,
        recurrence: opts.preserveRecurrenceZone
          ? request.recurrence
          : withCreatedRecurrenceZone(request.recurrence, request.all_day),
      }),
    ),
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
  sendCancellations = false,
): Promise<void> => {
  await CalFfi.addEventExdateJson(id, occurrence, calendarId, sendCancellations);
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
