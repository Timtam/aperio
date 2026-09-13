// Synthetic birthday-layer calendars (DESIGN §10.3): the Host emits one
// read-only calendar per contact list that has birthdays, with a stable id
// `aperio-birthdays:<contactListId>`, and one all-day event per contact and
// year, `aperio-birthday:<contactId>:<year>`. The prefixes below are the JS
// twins of `host_core::birthdays::{BIRTHDAY_CALENDAR_PREFIX,
// BIRTHDAY_EVENT_PREFIX}`, pinned against them by
// `shared/contracts/birthdayIds.json`, so the UI can recognise a birthday
// calendar (e.g. to skip a "Manage" affordance — a synthetic row has no real
// backing, so rename/colour/delete don't apply) or a birthday event (the
// editors open a read-only summary) where only an id is at hand.

import type { BirthdayLayer } from './types';

/** Prefix the Host uses for synthetic birthday-layer calendar ids. Mirrors
 *  `host_core::birthdays::BIRTHDAY_CALENDAR_PREFIX`. */
export const BIRTHDAY_CALENDAR_PREFIX = 'aperio-birthdays:';

/** Prefix of a synthetic birthday EVENT id (`aperio-birthday:<contact>:<year>`,
 *  singular — see `host_core::birthdays::BIRTHDAY_EVENT_PREFIX`). Note the
 *  missing "s" against {@link BIRTHDAY_CALENDAR_PREFIX}: the calendar is
 *  plural, the event is not. */
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

/**
 * A listed calendar with the name the user sees, in the UI language.
 *
 * A birthday calendar reaches the frontend named after its contact list alone
 * ("Familie"), with the row's `birthdays` layer saying what it is. The name
 * the user sees — "Geburtstage – Familie" — is built HERE, from the key
 * `birthdays.calendarName` and the list name, at the single loading boundary
 * of each surface (both `listCalendars` wrappers), so every consumer (sidebar,
 * pickers, chip labels) inherits it without knowing the rule. The core used to
 * stamp English "Birthdays – " into the name for this module to cut off
 * again; it answers the list and the kind now (DESIGN §4.5 a).
 *
 * Every other calendar comes back as it is — the same object.
 */
export function localizeBirthdayCalendarName<
  C extends { name: string; birthdays?: BirthdayLayer },
>(calendar: C, t: (key: string, vars: { list: string }) => string): C {
  if (!calendar.birthdays) return calendar;
  return {
    ...calendar,
    name: t('birthdays.calendarName', { list: calendar.birthdays.list_name }),
  };
}
