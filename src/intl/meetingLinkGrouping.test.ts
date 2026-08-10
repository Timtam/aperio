import { describe, expect, it } from 'vitest';

import {
  findMeetingLinkPairs,
  normalizeJoinUrl,
  type EventGroup,
  type SuggestionDecline,
} from '@aperio/shared';

interface Row {
  id: string;
  calendar_id: string;
  location: string | null;
  description: string | null;
}

const row = (id: string, calendar: string, url: string | null): Row => ({
  id,
  calendar_id: calendar,
  location: url,
  description: null,
});

const seriesId = (e: Row) => e.id;

/** A group over the given (calendar, event) pairs. */
function group(...members: [string, string][]): EventGroup {
  return {
    id: `g-${members.map(([, id]) => id).join('-')}`,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    members: members.map(([calendar_id, event_id]) => ({
      calendar_id,
      event_id,
      title: event_id,
      starts_at: '2026-01-01T09:00:00Z',
      added_at: '2026-01-01T00:00:00Z',
    })),
  };
}

const decline = (
  a: [string, string],
  b: [string, string],
): SuggestionDecline => ({
  calendar_a: a[0],
  event_a: a[1],
  calendar_b: b[0],
  event_b: b[1],
  declined_at: '2026-01-01T00:00:00Z',
});

// A real-shaped Webex join link: `j.php?MTID=` is what `classify` accepts,
// and it is the spelling that reads the same in all 26 languages of the
// invitation template.
const LINK = 'https://example.webex.com/example/j.php?MTID=m1234abcd';

describe('grouping a meeting with its appointment', () => {
  it('pairs the meeting row with the calendar entry that carries the same link', () => {
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [],
      [],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
    expect(pairs[0].meeting.id).toBe('m1');
    expect(pairs[0].event.id).toBe('e1');
  });

  it('does nothing when the link identifies more than one appointment', () => {
    // A standing meeting room reused by two entries: the link stops being an
    // identity and becomes a resemblance again, which is exactly what this
    // feature must not act on.
    const pairs = findMeetingLinkPairs(
      [
        row('m1', 'acc::meetings', LINK),
        row('e1', 'work', LINK),
        row('e2', 'private', LINK),
      ],
      [],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('does nothing when two meeting rows carry the same link', () => {
    const pairs = findMeetingLinkPairs(
      [
        row('m1', 'acc-a::meetings', LINK),
        row('m2', 'acc-b::meetings', LINK),
        row('e1', 'work', LINK),
      ],
      [],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('leaves a meeting without a calendar entry alone', () => {
    // The very reason the meetings calendar exists — it must stay visible on
    // its own, not wait for a partner.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK)],
      [],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('says nothing about rows with no link at all', () => {
    const pairs = findMeetingLinkPairs(
      [row('e1', 'work', null), row('e2', 'private', null)],
      [],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('does not repeat what is already grouped', () => {
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [group(['acc::meetings', 'm1'], ['work', 'e1'])],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('refuses to merge two groups the user built', () => {
    // `group_events` rejects this too: merging two claims about what an
    // appointment is cannot be inferred from a request nobody made.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [
        group(['acc::meetings', 'm1'], ['other', 'x']),
        group(['work', 'e1'], ['other', 'y']),
      ],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('adds the second one to the group the first is already in', () => {
    // One side grouped is the natural "and this one too", and that is what
    // `group_events` does with it.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [group(['work', 'e1'], ['private', 'e9'])],
      [],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
  });

  it('obeys a refusal the user has already made', () => {
    // Taking the meeting out of the group writes exactly this. Without it the
    // pair would come back on the next render — the one failure that would
    // make the feature a daily nuisance.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [],
      [decline(['acc::meetings', 'm1'], ['work', 'e1'])],
      seriesId,
    );
    expect(pairs).toEqual([]);
    // Named the other way round it is still the same decision.
    expect(
      findMeetingLinkPairs(
        [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
        [],
        [decline(['work', 'e1'], ['acc::meetings', 'm1'])],
        seriesId,
      ),
    ).toEqual([]);
  });

  it('groups a recurring pair once, however many days are in view', () => {
    // A range renders one row per occurrence; the membership is the series
    // master's, so the same pair would otherwise be handed over five times.
    const days = ['1', '2', '3'].flatMap(() => [
      row('m1', 'acc::meetings', LINK),
      row('e1', 'work', LINK),
    ]);
    expect(findMeetingLinkPairs(days, [], [], seriesId)).toHaveLength(0);
    // (Three occurrences of each make the link ambiguous, which is refused —
    // so a view hands in one day at a time, exactly like the folding rule.)
    expect(
      findMeetingLinkPairs(
        [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
        [],
        [],
        seriesId,
      ),
    ).toHaveLength(1);
  });

  it('sees through the spellings of one link, but not through its query', () => {
    // Host case and a trailing slash are noise. (The SCHEME never reaches here
    // in another case: `extractUrls` only recognises a lowercase `https://`,
    // so a link spelled `HTTPS://` is not detected as a meeting at all —
    // upstream of this, and the same for the filter.)
    const pairs = findMeetingLinkPairs(
      [
        row('m1', 'acc::meetings', 'https://Example.Webex.com/example/j.php?MTID=m1234abcd'),
        row('e1', 'work', LINK),
      ],
      [],
      [],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
    // …the query is not: the meeting id and its password live there.
    expect(
      findMeetingLinkPairs(
        [
          row('m1', 'acc::meetings', LINK),
          row('e1', 'work', 'https://example.webex.com/example/j.php?MTID=mZZZZ'),
        ],
        [],
        [],
        seriesId,
      ),
    ).toEqual([]);
  });
});

describe('normalizeJoinUrl', () => {
  it('folds case, space and a trailing slash', () => {
    expect(normalizeJoinUrl('  HTTPS://Example.COM/j/1/  ')).toBe(
      'https://example.com/j/1',
    );
  });

  it('keeps a string it cannot parse exactly as it stands', () => {
    expect(normalizeJoinUrl('not a url')).toBe('not a url');
    expect(normalizeJoinUrl('   ')).toBe('');
  });
});
