import { describe, expect, it } from 'vitest';

import {
  findGroupSuggestions,
  suggestionPairKey,
  type EventGroup,
  type SuggestionDecline,
} from '@aperio/shared';

const ev = (id: string, calendar_id: string, title: string, start: string, all_day = false) => ({
  id,
  calendar_id,
  title,
  start,
  all_day,
});

const seriesId = (e: { id: string }) => e.id;

const decline = (
  a: [string, string],
  b: [string, string],
): SuggestionDecline => ({
  calendar_a: a[0],
  event_a: a[1],
  calendar_b: b[0],
  event_b: b[1],
  declined_at: '2026-08-09T12:00:00Z',
});

const day = [
  ev('ev-a', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z'),
  ev('ev-b', 'private', 'Wochenplanung', '2026-08-10T08:00:00Z'),
  ev('ev-x', 'work', 'Zahnarzt', '2026-08-10T11:00:00Z'),
];

describe('offering a group nobody asked for', () => {
  it('never offers a videoconference meeting on a resemblance', () => {
    // A meeting carries the join URL its provider issued — an identity — and
    // `findMeetingLinkPairs` pairs it on that. Offering it here asks the user
    // the wrong question, and their answer is kept forever: the refusal names
    // the meeting, and the office full of "Team meeting" at 10:00 that this
    // module warns about is exactly where a meeting meets a stranger.
    const withMeeting = [
      ev('vc::m1', 'acc::meetings', 'Team meeting', '2026-08-10T10:00:00Z'),
      ev('ev-strange', 'shared-team', 'Team meeting', '2026-08-10T10:00:00Z'),
    ];
    expect(findGroupSuggestions(withMeeting, [], [], seriesId)).toEqual([]);
  });


  it('spots the copy in another calendar', () => {
    const found = findGroupSuggestions(day, [], [], seriesId);
    expect(found).toHaveLength(1);
    expect(found[0].first.id).toBe('ev-a');
    expect(found[0].second.id).toBe('ev-b');
  });

  it('says nothing about events that are already grouped', () => {
    const group: EventGroup = {
      id: 'g1',
      created_at: '2026-08-09T12:00:00Z',
      updated_at: '2026-08-09T12:00:00Z',
      members: [
        {
          calendar_id: 'work',
          event_id: 'ev-a',
          title: 'Wochenplanung',
          starts_at: '2026-08-10T08:00:00Z',
          added_at: '2026-08-09T12:00:00Z',
        },
        {
          calendar_id: 'private',
          event_id: 'ev-b',
          title: 'Wochenplanung',
          starts_at: '2026-08-10T08:00:00Z',
          added_at: '2026-08-09T12:00:00Z',
        },
      ],
    };
    expect(findGroupSuggestions(day, [group], [], seriesId)).toEqual([]);
  });

  it('never asks again about a pair that was declined', () => {
    // The whole reason the decline is stored: told once, Aperio has to stop.
    const declines = [decline(['work', 'ev-a'], ['private', 'ev-b'])];
    expect(findGroupSuggestions(day, [], declines, seriesId)).toEqual([]);
  });

  it('treats a decline the same way round as the other', () => {
    // Declined from B's side; offering it again from A's would be the same
    // question asked twice.
    const declines = [decline(['private', 'ev-b'], ['work', 'ev-a'])];
    expect(findGroupSuggestions(day, [], declines, seriesId)).toEqual([]);
  });

  it('offers each event at most once', () => {
    const three = [
      ev('ev-a', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z'),
      ev('ev-b', 'private', 'Wochenplanung', '2026-08-10T08:00:00Z'),
      ev('ev-c', 'colleague', 'Wochenplanung', '2026-08-10T08:00:00Z'),
    ];
    const found = findGroupSuggestions(three, [], [], seriesId);
    // One offer, not three pairings of the same appointment — answering it
    // groups a+b, and the next round offers c against that group's member.
    expect(found).toHaveLength(1);
  });

  it('keeps the strict match: not the same calendar, not another time', () => {
    expect(
      findGroupSuggestions(
        [
          ev('ev-a', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z'),
          ev('ev-dup', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z'),
          ev('ev-late', 'private', 'Wochenplanung', '2026-08-10T09:00:00Z'),
        ],
        [],
        [],
        seriesId,
      ),
    ).toEqual([]);
  });

  it('builds the same pair key whichever way round it is asked', () => {
    const a = { calendar_id: 'work', event_id: 'ev-a' };
    const b = { calendar_id: 'private', event_id: 'ev-b' };
    expect(suggestionPairKey(a, b)).toBe(suggestionPairKey(b, a));
  });
});
