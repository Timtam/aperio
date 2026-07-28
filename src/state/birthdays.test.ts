import { describe, expect, it } from 'vitest';

import {
  BIRTHDAY_CALENDAR_PREFIX,
  BIRTHDAY_EVENT_PREFIX,
  isBirthdayCalendarId,
  isBirthdayEventId,
} from '@aperio/shared';

// The two prefixes differ by a single letter ("birthdays:" for the synthetic
// calendar, "birthday:" for the events inside it — see host_core::birthdays).
// Both editors short-circuit on them, so a mixed-up prefix would silently let a
// contact-derived event open a form whose save can only fail.
describe('birthday id prefixes', () => {
  it('recognises a synthetic birthday CALENDAR id', () => {
    expect(isBirthdayCalendarId(`${BIRTHDAY_CALENDAR_PREFIX}list-1234`)).toBe(
      true,
    );
    expect(isBirthdayCalendarId('caldav-calendar-1')).toBe(false);
  });

  it('recognises a synthetic birthday EVENT id', () => {
    expect(isBirthdayEventId(`${BIRTHDAY_EVENT_PREFIX}contact-7:2026`)).toBe(
      true,
    );
    expect(isBirthdayEventId('local-event-42')).toBe(false);
  });

  it('keeps the calendar and event prefixes apart', () => {
    // A calendar id must NOT read as an event id, or the birthday calendar
    // itself would short-circuit surfaces meant for its events.
    expect(BIRTHDAY_CALENDAR_PREFIX.startsWith(BIRTHDAY_EVENT_PREFIX)).toBe(
      false,
    );
    expect(isBirthdayCalendarId(`${BIRTHDAY_EVENT_PREFIX}contact-7:2026`)).toBe(
      false,
    );
  });
});
