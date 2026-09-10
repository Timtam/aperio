import { describe, expect, it } from 'vitest';

import {
  eventGroupMemberKey,
  indexEventGroups,
  memberFromEvent,
  type EventGroup,
} from '@aperio/shared';

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
      added_at: '2026-08-09T12:00:01Z',
    },
  ],
};

describe('event groups', () => {
  // Asked through `index.get(eventGroupMemberKey(...))` — the way every
  // production caller asks — rather than through the `groupForEvent` wrapper
  // that used to sit on top. The wrapper had no callers and is gone; the RULE
  // it was testing is the key's, and that is what this pins.
  it('finds a row by calendar AND id, not by id alone', () => {
    const index = indexEventGroups([group]);
    expect(index.get(eventGroupMemberKey('work', 'ev-a'))?.id).toBe('g1');
    // The same event id in a calendar that is NOT a member. Provider ids are
    // only unique within a calendar, so keying on the id alone would claim a
    // stranger belongs to the group.
    expect(index.get(eventGroupMemberKey('colleague', 'ev-a'))).toBeUndefined();
  });

  it('keeps two members apart when a separator moves', () => {
    // Why the key is JSON and not a joined string: a provider id may contain
    // any character, so `("a b", "c")` and `("a", "b c")` must not collide. A
    // collision here claims a stranger belongs to a group.
    expect(eventGroupMemberKey('a b', 'c')).not.toBe(
      eventGroupMemberKey('a', 'b c'),
    );
  });

  it('takes the signature from the event as it is now', () => {
    expect(
      memberFromEvent({
        id: 'ev-a',
        calendar_id: 'work',
        title: 'Wochenplanung',
        start: '2026-08-10T08:00:00Z',
      }),
    ).toEqual({
      calendar_id: 'work',
      event_id: 'ev-a',
      title: 'Wochenplanung',
      starts_at: '2026-08-10T08:00:00Z',
    });
    // A titleless event still yields a usable member: the signature is a way
    // back to the event, not a requirement for joining.
    expect(
      memberFromEvent({ id: 'ev-a', calendar_id: 'work' }).title,
    ).toBe('');
  });
});
