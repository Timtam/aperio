import { describe, expect, it } from 'vitest';

import {
  askOutcome,
  grantedOutcome,
  missingStores,
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

describe('what the settings still have to change', () => {
  it('names the stores Aperio may not read', () => {
    expect(missingStores(after('full', 'full'))).toBe('none');
    expect(missingStores(after('denied', 'full'))).toBe('calendars');
    // "Add events only" reads nothing (decision 171).
    expect(missingStores(after('write_only', 'full'))).toBe('calendars');
    expect(missingStores(after('full', 'denied'))).toBe('reminders');
    expect(missingStores(after('denied', 'restricted'))).toBe('both');
    expect(missingStores(after('denied', null))).toBe('calendars');
  });
});
