import { describe, expect, it } from 'vitest';

import contract from '../../shared/contracts/calendarDefaultReminders.json';
import { calendarDefaultRemindersKey, madeNoReminderChoice } from '@aperio/shared';
import type { DefaultReminder } from '@aperio/shared';

/**
 * The TypeScript half of the calendar-default-reminders wire contract.
 *
 * Its Rust twin lives in `crates/host-core/src/reminders.rs` and reads the
 * SAME file. That is the point: the two languages cannot share a type, so they
 * share a fixture, and a rename on either side now fails a test instead of
 * going quiet.
 *
 * Quiet is what makes this worth pinning. The Rust reader parses the stored
 * list all-or-nothing and falls back to an empty one, so a single field
 * TypeScript spells differently takes every default reminder of that calendar
 * with it — while the settings panel still shows them, because it reads back
 * its own writer. Nothing short of creating an appointment on a real account
 * and waiting for a phone to stay silent would reveal it.
 */

interface ContractEntry {
  kind: string;
  minutes_before?: number;
  at?: string;
  attach: boolean;
  hasSound: boolean;
}

/** What the entry says, in the same terms the Rust twin checks. */
function describeEntry(entry: DefaultReminder): ContractEntry {
  return {
    kind: entry.kind.type,
    ...('minutes_before' in entry.kind
      ? { minutes_before: entry.kind.minutes_before }
      : {}),
    ...('at' in entry.kind ? { at: entry.kind.at } : {}),
    attach: entry.attach === true,
    hasSound: entry.sound != null,
  };
}

describe('calendar default reminders → the pref key', () => {
  it('is built the way the contract says', () => {
    const { calendarId, key } = contract.prefKey.example;
    expect(calendarDefaultRemindersKey(calendarId)).toBe(key);
  });

  it('still starts with the prefix that puts it on the sync whitelist', () => {
    // A key outside `calendar.` saves locally and never reaches the user's
    // other devices — a failure that looks exactly like "it worked".
    expect(calendarDefaultRemindersKey('anything')).toMatch(
      new RegExp(`^${contract.prefKey.syncedUnderPrefix.replace('.', '\\.')}`),
    );
  });
});

describe('calendar default reminders → what TypeScript stores', () => {
  for (const sample of contract.samples) {
    it(`reads ${sample.name} as the contract describes it`, () => {
      const parsed = JSON.parse(sample.stored) as DefaultReminder[];
      expect(parsed.map(describeEntry)).toEqual(sample.entries);
    });

    it(`writes ${sample.name} back byte for byte`, () => {
      // The round trip has to be exact, not merely equivalent: the Rust side
      // reads TEXT, and a re-ordered or re-spelled field is a different
      // contract even when it means the same thing to JavaScript.
      const parsed = JSON.parse(sample.stored) as DefaultReminder[];
      expect(JSON.stringify(parsed)).toBe(sample.stored);
    });
  }
});

describe('calendar default reminders → "no reminder choice was made"', () => {
  it('is true only for an editor nobody touched that carries nothing', () => {
    // The rule both editors now ask instead of each spelling it out. It is the
    // only thing that lets a calendar's attached default reach the
    // appointment, and getting it wrong is silent on both platforms.
    expect(madeNoReminderChoice(false, [])).toBe(true);
  });

  it('is false once the user has touched the reminders', () => {
    // An emptied list is a decision — "no reminder" — and must not be refilled.
    expect(madeNoReminderChoice(true, [])).toBe(false);
  });

  it('is false when the appointment carries reminders of its own', () => {
    expect(madeNoReminderChoice(false, [{}])).toBe(false);
    expect(madeNoReminderChoice(true, [{}])).toBe(false);
  });
});
