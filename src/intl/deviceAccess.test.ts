import { describe, expect, it } from 'vitest';

import {
  answerOutcome,
  askOutcome,
  gainedAccess,
  grantedOutcome,
  missingStores,
  restrictedStores,
  settingsStores,
  type OsAccessReport,
} from '@aperio/shared';

/** The report read after the prompt. */
const after = (calendar: string, tasks: string | null): OsAccessReport =>
  ({ account_names: ['Dieses Gerät'], calendar, tasks, ask_now: null }) as OsAccessReport;

const BOTH = { events: true, reminders: true };

describe('what came of asking for the device calendars at start', () => {
  it('says granted when everything asked about is granted', () => {
    expect(askOutcome(BOTH, after('full', 'full'))).toBe('granted');
  });

  it('names the half that was granted', () => {
    expect(askOutcome(BOTH, after('full', 'denied'))).toBe('calendarsOnly');
    expect(askOutcome(BOTH, after('write_only', 'full'))).toBe('remindersOnly');
  });

  it('says not granted when nothing asked about is granted', () => {
    expect(askOutcome(BOTH, after('denied', 'denied'))).toBe('denied');
    // "Add events only" reads nothing, so it is no grant (decision 171).
    expect(askOutcome({ events: true, reminders: false }, after('write_only', null))).toBe(
      'denied',
    );
  });

  it('does not credit the prompt with a grant it did not ask for', () => {
    // The calendars were already full; only the reminders were asked about.
    expect(askOutcome({ events: false, reminders: true }, after('full', 'denied'))).toBe(
      'denied',
    );
    expect(askOutcome({ events: false, reminders: true }, after('full', 'full'))).toBe(
      'granted',
    );
  });
});

describe('what is said after a prompt', () => {
  it('names a store refused before the question as still missing', () => {
    // Calendars refused earlier, reminders never asked: the prompt asks only
    // the reminders, and "access granted" would hide the calendars.
    const onlyReminders = { events: false, reminders: true };
    expect(answerOutcome(onlyReminders, after('denied', 'full'))).toBe('remindersOnly');
    expect(answerOutcome(onlyReminders, after('full', 'full'))).toBe('granted');
    expect(answerOutcome(onlyReminders, after('full', 'denied'))).toBe('denied');
    expect(answerOutcome(BOTH, after('full', 'denied'))).toBe('calendarsOnly');
  });
});

describe('a grant made in the OS settings', () => {
  it('counts every store Aperio can read again', () => {
    expect(grantedOutcome(after('full', 'full'))).toBe('granted');
    expect(grantedOutcome(after('full', 'denied'))).toBe('calendarsOnly');
    expect(grantedOutcome(after('write_only', 'full'))).toBe('remindersOnly');
  });

  it('is whole on a platform without reminders', () => {
    // Android: the calendars are all there is.
    expect(grantedOutcome(after('full', null))).toBe('granted');
    expect(grantedOutcome(after('undetermined', null))).toBe('denied');
  });
});

describe('which stores a sentence names', () => {
  it('names the stores Aperio may not read', () => {
    expect(missingStores(after('full', 'full'))).toBe('none');
    expect(missingStores(after('denied', 'full'))).toBe('calendars');
    // "Add events only" reads nothing (decision 171).
    expect(missingStores(after('write_only', 'full'))).toBe('calendars');
    expect(missingStores(after('full', 'denied'))).toBe('reminders');
    expect(missingStores(after('denied', 'restricted'))).toBe('both');
    expect(missingStores(after('denied', null))).toBe('calendars');
  });

  it('sends to the settings only what they can change', () => {
    // A policy's store is not Aperio's settings to change, and a store the
    // OS still asks about is asked about first.
    expect(settingsStores(after('restricted', 'denied'))).toBe('reminders');
    expect(restrictedStores(after('restricted', 'denied'))).toBe('calendars');
    expect(settingsStores(after('write_only', 'not_asked'))).toBe('calendars');
    expect(settingsStores(after('denied', 'write_only'))).toBe('both');
    expect(restrictedStores(after('denied', 'write_only'))).toBe('none');
    // Android: the platform cannot say, and only the settings can grant
    // after "don't ask again".
    expect(settingsStores(after('undetermined', null))).toBe('calendars');
  });

  it('tells a store that became readable', () => {
    expect(gainedAccess(after('denied', 'full'), after('full', 'full'))).toBe(true);
    expect(gainedAccess(after('full', 'denied'), after('full', 'full'))).toBe(true);
    expect(gainedAccess(after('full', 'denied'), after('full', 'denied'))).toBe(false);
    // A loss is no gain.
    expect(gainedAccess(after('full', 'full'), after('denied', 'full'))).toBe(false);
  });
});
