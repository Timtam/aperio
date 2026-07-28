// Synthetic birthday-layer calendars (DESIGN §10.3): the Host emits one
// read-only calendar per contact list that has birthdays, with a stable
// id `aperio-birthdays:<contactListId>`. This is the JS twin of
// `host_core::birthdays::{BIRTHDAY_CALENDAR_PREFIX, is_birthday_calendar_id}`,
// so the UI can recognise a birthday calendar (e.g. to skip a "Manage"
// affordance — a synthetic row has no real backing, so rename/colour/delete
// don't apply).

/** Prefix the Host uses for synthetic birthday-layer calendar ids. Mirrors
 *  `host_core::birthdays::BIRTHDAY_CALENDAR_PREFIX`. */
export const BIRTHDAY_CALENDAR_PREFIX = 'aperio-birthdays:';

/** Prefix of a synthetic birthday EVENT id (`aperio-birthday:<contact>:<year>`,
 *  singular — see `host_core::birthdays`). Note the missing "s" against
 *  {@link BIRTHDAY_CALENDAR_PREFIX}: the calendar is plural, the event is not. */
export const BIRTHDAY_EVENT_PREFIX = 'aperio-birthday:';

/** True for a synthetic birthday-layer calendar id. */
export function isBirthdayCalendarId(id: string): boolean {
  return id.startsWith(BIRTHDAY_CALENDAR_PREFIX);
}

/** True for a synthetic birthday EVENT id. Such an event is derived from a
 *  contact's birthday and has no row behind it, so the editors short-circuit
 *  to a read-only summary instead of offering fields whose save would fail. */
export function isBirthdayEventId(id: string): boolean {
  return id.startsWith(BIRTHDAY_EVENT_PREFIX);
}
