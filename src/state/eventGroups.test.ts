import { describe, expect, it } from 'vitest';

import {
  groupForEvent,
  indexEventGroups,
  memberFromEvent,
  otherMemberCount,
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
  it('finds a row by calendar AND id, not by id alone', () => {
    const index = indexEventGroups([group]);
    expect(groupForEvent(index, { calendar_id: 'work', id: 'ev-a' })?.id).toBe(
      'g1',
    );
    // The same event id in a calendar that is NOT a member. Provider ids are
    // only unique within a calendar, so keying on the id alone would claim a
    // stranger belongs to the group.
    expect(
      groupForEvent(index, { calendar_id: 'colleague', id: 'ev-a' }),
    ).toBeUndefined();
    expect(groupForEvent(index, { calendar_id: 'work', id: null })).toBeUndefined();
  });

  it('counts the OTHERS, which is what a row announces', () => {
    expect(otherMemberCount(group, { calendar_id: 'work', id: 'ev-a' })).toBe(1);
    // An event that is not in the group at all sees every member as another.
    expect(otherMemberCount(group, { calendar_id: 'x', id: 'ev-z' })).toBe(2);
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
