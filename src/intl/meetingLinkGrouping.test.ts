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
    // A standing meeting room reused by two unrelated entries: the link stops
    // being an identity and becomes a resemblance again, which is exactly what
    // this feature must not act on.
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

  it('joins the group when every copy carrying the link is in it', () => {
    // The leading case of this whole feature: one appointment kept in several
    // calendars, each copy carrying the same forwarded join link. Counting the
    // ROWS would call that ambiguous and refuse — refusing precisely for an
    // appointment the user has already declared to be one thing. The group IS
    // the answer to "which appointment is this meeting?".
    const pairs = findMeetingLinkPairs(
      [
        row('m1', 'acc::meetings', LINK),
        row('e1', 'work', LINK),
        row('e2', 'private', LINK),
      ],
      [group(['work', 'e1'], ['private', 'e2'])],
      [],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
    expect(pairs[0].meeting.id).toBe('m1');
    // Either copy names the group, and `group_events` joins it rather than
    // starting a second one.
    expect(pairs[0].event.calendar_id).toBe('work');
  });

  it('still refuses when one claimant is outside the group', () => {
    // The rule is "count appointments, not rows" — not "ignore the extras".
    const pairs = findMeetingLinkPairs(
      [
        row('m1', 'acc::meetings', LINK),
        row('e1', 'work', LINK),
        row('e2', 'private', LINK),
        row('e3', 'somebody-else', LINK),
      ],
      [group(['work', 'e1'], ['private', 'e2'])],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('leaves a meeting that already belongs to an appointment alone', () => {
    // The hole the counting rule had: it counts the claimants IN VIEW, and the
    // appointment the meeting is already grouped with is not one of them. A
    // second, unrelated appointment claiming the same link would have been
    // merged into that group — and which appointments got merged would have
    // depended on nothing but the calendars that happened to be switched on.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e2', 'private', LINK)],
      [group(['acc::meetings', 'm1'], ['work', 'e1'])],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('obeys a refusal naming a copy that is not in view', () => {
    // Taking the meeting out wrote a mark against every member it left. A copy
    // added to the appointment afterwards has no mark of its own, so asking
    // only the rows on screen makes the refusal invisible exactly when that
    // younger copy is the one being rendered.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e2', 'private', LINK)],
      [group(['work', 'e1'], ['private', 'e2'])],
      [decline(['acc::meetings', 'm1'], ['work', 'e1'])],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('a refusal about a DIFFERENT partner leaves this meeting pairable', () => {
    // The reason the one-sided read was reverted. `ungroup` writes a mark
    // between the departing event and every member it left, so a meeting that
    // merely sat in that group gets named by a statement about somebody else.
    // Reading a mark by its meeting half alone turned that into a permanent,
    // global refusal to pair the meeting with anything — over a refusal the
    // user never made about it.
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [],
      [decline(['acc::meetings', 'm1'], ['private', 'someone-else'])],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
  });

  it('a refusal about a different meeting blocks nothing here', () => {
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [],
      [decline(['acc::meetings', 'm-other'], ['work', 'e1'])],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
  });

  it('a refusal the user took back blocks nothing', () => {
    // The host filters cleared rows before handing them over; this guards a
    // caller feeding raw snapshot rows. The later statement wins.
    const takenBack = {
      ...decline(['acc::meetings', 'm1'], ['work', 'e1']),
      cleared_at: '2026-02-01T00:00:00Z',
    };
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), row('e1', 'work', LINK)],
      [],
      [takenBack],
      seriesId,
    );
    expect(pairs).toHaveLength(1);
  });

  it('adds one meeting per account, however often the provider remints its id', () => {
    // Webex lists a recurring meeting as one row per occurrence, each with its
    // own id, and its list response carries no series id to collapse them by.
    // Today's row is not yesterday's — so without this rule the day view would
    // add a new member every morning and the count would climb forever.
    const pairs = findMeetingLinkPairs(
      [
        row('m-tuesday', 'acc::meetings', LINK),
        row('e1', 'work', LINK),
      ],
      [group(['work', 'e1'], ['acc::meetings', 'm-monday'])],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
  });

  it('obeys a refusal made against any copy of the appointment', () => {
    // Taking the meeting out of the group wrote a mark against EVERY member it
    // left. Checking only the first copy would let the pair back in as soon as
    // the rows arrived in another order.
    const pairs = findMeetingLinkPairs(
      [
        row('m1', 'acc::meetings', LINK),
        row('e1', 'work', LINK),
        row('e2', 'private', LINK),
      ],
      [group(['work', 'e1'], ['private', 'e2'])],
      [decline(['acc::meetings', 'm1'], ['private', 'e2'])],
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

  it('counts a series once, however many days are in view', () => {
    // A range renders one row per occurrence, and membership is keyed by the
    // series master — so three days of one appointment are one appointment,
    // not three claimants. Counting rows would have made the same data
    // ambiguous in the week view and fine in the day view, which is a property
    // of the open range rather than of the data.
    //
    // (These rows have no RRULE of their own: this is one appointment listed
    // three times, which is what the deduplication is about. A genuinely
    // recurring appointment is refused outright — see below.)
    const days = ['1', '2', '3'].flatMap(() => [
      row('m1', 'acc::meetings', LINK),
      row('e1', 'work', LINK),
    ]);
    const pairs = findMeetingLinkPairs(days, [], [], seriesId);
    expect(pairs).toHaveLength(1);
    expect(pairs[0].event.id).toBe('e1');
  });

  it('leaves a recurring appointment alone', () => {
    // A group's members are series. Webex has no series for a meeting row to
    // name — it lists one row per occurrence — and a meeting that does not
    // recur while the appointment does really is absent on most days. Either
    // way the group would claim, on every day but one, a copy that is not
    // there.
    const weekly = {
      ...row('e1', 'work', LINK),
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=TU' },
    };
    const pairs = findMeetingLinkPairs(
      [row('m1', 'acc::meetings', LINK), weekly],
      [],
      [],
      seriesId,
    );
    expect(pairs).toEqual([]);
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
