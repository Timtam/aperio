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

/** True for a synthetic birthday-layer calendar id. */
export function isBirthdayCalendarId(id: string): boolean {
  return id.startsWith(BIRTHDAY_CALENDAR_PREFIX);
}
