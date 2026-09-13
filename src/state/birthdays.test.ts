import { describe, expect, it } from 'vitest';

import {
  BIRTHDAY_CALENDAR_PREFIX,
  BIRTHDAY_EVENT_PREFIX,
  isBirthdayCalendarId,
  isBirthdayEventId,
  localizeBirthdayCalendarName,
} from '@aperio/shared';

import contract from '../../shared/contracts/birthdayIds.json';
import de from '../../locales/de/translation.json';
import en from '../../locales/en/translation.json';

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

// The TypeScript half of `shared/contracts/birthdayIds.json`; the Rust half is
// `host_core::birthdays::tests::id_contract`, reading the same file and
// checking the ids the synthesis actually mints.
describe('birthday ids — spelled as the host mints them', () => {
  it('uses the same prefixes', () => {
    expect(BIRTHDAY_CALENDAR_PREFIX).toBe(contract.calendarPrefix);
    expect(BIRTHDAY_EVENT_PREFIX).toBe(contract.eventPrefix);
  });

  it('recognises the ids the host mints', () => {
    expect(isBirthdayCalendarId(contract.examples.calendarId)).toBe(true);
    expect(isBirthdayEventId(contract.examples.eventId)).toBe(true);
    expect(isBirthdayEventId(contract.examples.calendarId)).toBe(false);
  });
});

describe('a birthday calendar is named on this side', () => {
  // A recording `t`: the key and the list it was asked with, so the test
  // sees WHAT is looked up rather than one language's words.
  const t = (key: string, vars: { list: string }) => `${key}(${vars.list})`;

  it('builds the name from the key and the list the row carries', () => {
    const row = {
      id: 'aperio-birthdays:list-1234',
      name: 'Familie',
      birthdays: { contact_list_id: 'list-1234', list_name: 'Familie' },
    };
    expect(localizeBirthdayCalendarName(row, t).name).toBe('birthdays.calendarName(Familie)');
  });

  it('leaves every other calendar as it is — the same object', () => {
    const row = { id: 'caldav-calendar-1', name: 'Arbeit' };
    expect(localizeBirthdayCalendarName(row, t)).toBe(row);
  });

  it('has the key in both catalogs, with the list in it', () => {
    expect(de.birthdays.calendarName).toContain('{{list}}');
    expect(en.birthdays.calendarName).toContain('{{list}}');
  });
});
