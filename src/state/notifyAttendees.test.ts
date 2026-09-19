import { describe, expect, it } from 'vitest';
import {
  attendeeNotice,
  cancellationNotice,
  notifierSentence,
  sendsInvitations,
} from '@aperio/shared';

// The rules both editors and every delete dialog share for telling a
// meeting's attendees (decisions 70a, 74a, 76a, 80a).
describe('attendeeNotice', () => {
  const offers = { supports_scheduling: true };
  const always = { supports_scheduling: true, always_notifies_attendees: true };
  const own = { attendees: ['bob@example.com'], organized_elsewhere: false };

  it('offers the choice for invitees on a calendar that can save silently', () => {
    expect(
      attendeeNotice({ calendar: offers, attendees: ['bob@example.com'], original: null }),
    ).toBe('offer');
    expect(
      attendeeNotice({ calendar: { supports_scheduling: false }, attendees: ['bob@example.com'], original: null }),
    ).toBe('none');
    expect(attendeeNotice({ calendar: null, attendees: ['bob@example.com'], original: null })).toBe('none');
  });

  it('says so instead where the provider always notifies (76a)', () => {
    expect(attendeeNotice({ calendar: always, attendees: ['bob@example.com'], original: own })).toBe(
      'always',
    );
  });

  it('keeps it when the edit removed every invitee (74a)', () => {
    expect(attendeeNotice({ calendar: offers, attendees: [], original: own })).toBe('offer');
    expect(attendeeNotice({ calendar: always, attendees: [], original: own })).toBe('always');
    // Nobody before, nobody now: nobody to tell.
    expect(attendeeNotice({ calendar: offers, attendees: [], original: { attendees: [] } })).toBe(
      'none',
    );
    expect(attendeeNotice({ calendar: offers, attendees: [], original: null })).toBe('none');
  });

  it('never for a meeting someone else organizes (70a)', () => {
    const elsewhere = { ...own, organized_elsewhere: true };
    expect(attendeeNotice({ calendar: offers, attendees: ['bob@example.com'], original: elsewhere })).toBe(
      'none',
    );
    expect(attendeeNotice({ calendar: always, attendees: ['bob@example.com'], original: elsewhere })).toBe(
      'none',
    );
  });
});

describe('sendsInvitations', () => {
  it('follows the checkbox only where there is one', () => {
    expect(sendsInvitations('offer', true)).toBe(true);
    expect(sendsInvitations('offer', false)).toBe(false);
    expect(sendsInvitations('always', false)).toBe(true);
    expect(sendsInvitations('none', true)).toBe(false);
  });
});

describe('cancellationNotice', () => {
  const meeting = { attendees: ['bob@example.com'], organized_elsewhere: false };
  it('asks only about a meeting the account organizes, with invitees (70a, 80a)', () => {
    expect(cancellationNotice({ supports_scheduling: true }, meeting)).toBe('offer');
    expect(
      cancellationNotice({ supports_scheduling: true, always_notifies_attendees: true }, meeting),
    ).toBe('always');
    expect(
      cancellationNotice({ supports_scheduling: true }, { ...meeting, organized_elsewhere: true }),
    ).toBe('none');
    expect(cancellationNotice({ supports_scheduling: true }, { attendees: [] })).toBe('none');
    expect(cancellationNotice({ supports_scheduling: false }, meeting)).toBe('none');
  });
});

describe('notifierSentence', () => {
  it('names the service when the adapter knows it', () => {
    expect(notifierSentence({ notifier_name: 'iCloud' }, 'change')).toEqual({
      key: 'dialogs.event.fields.notifyAlwaysNamed',
      values: { service: 'iCloud' },
    });
    expect(notifierSentence({}, 'cancellation')).toEqual({
      key: 'dialogs.deleteScope.notifyAlways',
      values: {},
    });
  });
});
