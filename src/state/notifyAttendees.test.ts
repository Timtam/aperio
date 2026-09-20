import { describe, expect, it } from 'vitest';
import {
  attendeeNotice,
  declineSentence,
  invitationLocked,
  cancellationNotice,
  notifierSentence,
  sendsInvitations,
  silentSentence,
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

describe('invitationLocked', () => {
  const invitation = { organized_elsewhere: true, attendees: ['me@example.com'] };

  it('is only someone else\u2019s meeting on a provider that takes no edits (77a)', () => {
    expect(invitationLocked({ invitations_reply_only: true }, invitation)).toBe(true);
    // The account's own meeting is editable, whatever the provider does.
    expect(
      invitationLocked({ invitations_reply_only: true }, { ...invitation, organized_elsewhere: false }),
    ).toBe(false);
    // A provider that keeps an invitee's edits leaves the editor open.
    expect(invitationLocked({ invitations_reply_only: false }, invitation)).toBe(false);
    expect(invitationLocked({}, invitation)).toBe(false);
    // Nothing known yet: not locked, and the adapter still refuses a write it
    // may not make.
    expect(invitationLocked(null, invitation)).toBe(false);
    expect(invitationLocked({ invitations_reply_only: true }, null)).toBe(false);
  });
});

describe('declineSentence', () => {
  it('says the organizer gets a decline, shaped like the notifier sentence (83b)', () => {
    expect(declineSentence()).toEqual({
      key: 'dialogs.deleteScope.organizerGetsDecline',
      values: {},
    });
  });

  it('says the organizer hears nothing where the server may not send (98)', () => {
    expect(declineSentence({ scheduling_silenced: true })).toEqual({
      key: 'dialogs.deleteScope.organizerNotTold',
      values: {},
    });
  });
});

/**
 * Decision 98, measured in live round 6: an invitation imported from a `.ics`
 * file carries RFC 6638 `SCHEDULE-AGENT=CLIENT`, and iCloud then sends
 * nothing at all — no reply to an answer, no cancellation on a delete. The
 * calendar's capability is about the CALENDAR; this one fact about the EVENT
 * overrules it, so no dialog promises a message nobody sends.
 */
describe('an event the server may not send for', () => {
  const always = { supports_scheduling: true, always_notifies_attendees: true, notifier_name: 'iCloud' };
  const offers = { supports_scheduling: true };
  const silenced = {
    attendees: ['bob@example.com'],
    organized_elsewhere: false,
    scheduling_silenced: true,
  };

  it('is its own notice, whatever the calendar can do', () => {
    expect(
      attendeeNotice({ calendar: always, attendees: ['bob@example.com'], original: silenced }),
    ).toBe('silent');
    // Even where the provider would otherwise offer the choice: asking to
    // notify would ask for something the server will not do.
    expect(
      attendeeNotice({ calendar: offers, attendees: ['bob@example.com'], original: silenced }),
    ).toBe('silent');
    // Without the marker the calendar decides again.
    expect(
      attendeeNotice({
        calendar: always,
        attendees: ['bob@example.com'],
        original: { ...silenced, scheduling_silenced: false },
      }),
    ).toBe('always');
    // Nobody to tell stays nobody to tell.
    expect(
      attendeeNotice({ calendar: always, attendees: [], original: { ...silenced, attendees: [] } }),
    ).toBe('none');
  });

  it('never asks the provider to send', () => {
    expect(sendsInvitations('silent', true)).toBe(false);
    expect(sendsInvitations('silent', false)).toBe(false);
  });

  it('says nobody is told, and names the door that stayed shut on a change', () => {
    expect(silentSentence(always, 'change')).toEqual({
      key: 'dialogs.event.fields.notifySilentNamed',
      values: { service: 'iCloud' },
    });
    expect(silentSentence({}, 'change')).toEqual({
      key: 'dialogs.event.fields.notifySilent',
      values: {},
    });
    // The cancellation sentence names nobody: there is nothing to name.
    expect(silentSentence(always, 'cancellation')).toEqual({
      key: 'dialogs.deleteScope.attendeesNotTold',
      values: {},
    });
  });
});
