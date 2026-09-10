import { describe, expect, it } from 'vitest';

import { withoutDuplicateMeetings } from '@aperio/shared';

const LINK = 'https://example.webex.com/example/j.php?MTID=mabc';

/** An event from a real calendar, carrying a meeting link in its text. */
const realEvent = {
  calendar_id: 'cal-1',
  location: '',
  description: `Join the meeting: ${LINK}`,
};

/** The same meeting as the videoconference account's own calendar lists it. */
const synthesized = {
  calendar_id: 'acc-1::meetings',
  location: LINK,
  description: null,
};

/** A meeting that exists only at the provider — created in its web UI. */
const providerOnly = {
  calendar_id: 'acc-1::meetings',
  location: 'https://example.webex.com/example/j.php?MTID=monly',
  description: null,
};

describe('withoutDuplicateMeetings', () => {
  it('keeps a meeting that has no calendar entry anywhere', () => {
    // The whole reason the meetings calendar exists.
    expect(withoutDuplicateMeetings([providerOnly])).toEqual([providerOnly]);
  });

  it('drops the synthesized copy when a real event already shows it', () => {
    const kept = withoutDuplicateMeetings([realEvent, synthesized]);
    expect(kept).toEqual([realEvent]);
  });

  it('is order-independent', () => {
    expect(withoutDuplicateMeetings([synthesized, realEvent])).toEqual([realEvent]);
  });

  it('never drops a real event, even two carrying the same link', () => {
    // The same meeting can legitimately appear in two calendars the user
    // subscribes to. That is the duplicate-event problem, not this one.
    const other = { ...realEvent, calendar_id: 'cal-2' };
    expect(withoutDuplicateMeetings([realEvent, other])).toEqual([realEvent, other]);
  });

  it('matches on the link and not on the wording around it', () => {
    // A German invitation and Aperio's own English block are the same meeting.
    const german = {
      calendar_id: 'cal-1',
      location: '',
      description: `Nehmen Sie an dieser Videokonferenz teil via ${LINK}`,
    };
    expect(withoutDuplicateMeetings([german, synthesized])).toEqual([german]);
  });

  it('leaves a synthesized event alone when nothing else claims its link', () => {
    expect(withoutDuplicateMeetings([realEvent, providerOnly])).toEqual([
      realEvent,
      providerOnly,
    ]);
  });

  it('returns the input untouched when no event carries a link at all', () => {
    const plain = [{ calendar_id: 'cal-1', location: '', description: 'Lunch' }];
    expect(withoutDuplicateMeetings(plain)).toBe(plain);
  });

  it('never drops a meeting that is already in a group', () => {
    // The folding is what hides it then, and it hides it while COUNTING it —
    // the row says "2×" and the group can be opened. Dropped here first, the
    // count would be a lie about a row that is not there.
    //
    // This case reaches the core through the `grouped` flag on the wire, which
    // is what the `isGrouped` callback becomes. Written because a sabotage
    // proved the rest of this file could not see it: removing the exception in
    // Rust turned the core's own test red and left every case here green.
    const kept = withoutDuplicateMeetings(
      [realEvent, synthesized],
      (ev) => ev === synthesized,
    );
    expect(kept).toEqual([realEvent, synthesized]);
  });
});
