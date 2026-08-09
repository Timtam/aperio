import { describe, expect, it } from 'vitest';

import { findHealableMembers, type EventGroup } from '@aperio/shared';

const range = {
  start: new Date('2026-08-10T00:00:00Z'),
  end: new Date('2026-08-11T00:00:00Z'),
};

const member = (calendar: string, event: string, starts = '2026-08-10T08:00:00Z') => ({
  calendar_id: calendar,
  event_id: event,
  title: 'Wochenplanung',
  starts_at: starts,
  added_at: '2026-08-09T12:00:00Z',
});

const group = (members: ReturnType<typeof member>[]): EventGroup => ({
  id: 'g1',
  created_at: '2026-08-09T12:00:00Z',
  updated_at: '2026-08-09T12:00:00Z',
  members,
});

const ev = (id: string, calendar_id: string, title: string, start: string, all_day = false) => ({
  id,
  calendar_id,
  title,
  start,
  all_day,
});

const seriesId = (e: { id: string }) => e.id;

describe('finding a member again after the provider re-minted its id', () => {
  it('recognises the same appointment under a new id', () => {
    const healed = findHealableMembers(
      [group([member('work', 'old-a'), member('private', 'ev-b')])],
      [
        ev('new-a', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z'),
        ev('ev-b', 'private', 'Wochenplanung', '2026-08-10T08:00:00Z'),
      ],
      range,
      seriesId,
    );
    expect(healed).toEqual([
      {
        group_id: 'g1',
        calendar_id: 'work',
        old_event_id: 'old-a',
        new_event_id: 'new-a',
      },
    ]);
  });

  it('leaves a member alone when it simply is not in this range', () => {
    // Not on screen is not the same as gone. Rewriting on that suspicion is
    // exactly the bug this guards against.
    const healed = findHealableMembers(
      [group([member('work', 'ev-a', '2026-09-01T08:00:00Z')])],
      [ev('other', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z')],
      range,
      seriesId,
    );
    expect(healed).toEqual([]);
  });

  it('leaves a member alone when nothing matching is there', () => {
    // It may be a copy the user deleted; quietly shrinking the group on that
    // suspicion changes something nobody asked to change.
    const healed = findHealableMembers(
      [group([member('work', 'gone')])],
      [ev('unrelated', 'work', 'Zahnarzt', '2026-08-10T08:00:00Z')],
      range,
      seriesId,
    );
    expect(healed).toEqual([]);
  });

  it('will not take a same-named event from another calendar', () => {
    // That is not the member coming back, it is a different copy — and
    // rewriting to it would move the group somewhere nobody pointed it.
    const healed = findHealableMembers(
      [group([member('work', 'gone')])],
      [ev('ev-b', 'private', 'Wochenplanung', '2026-08-10T08:00:00Z')],
      range,
      seriesId,
    );
    expect(healed).toEqual([]);
  });

  it('will not take an event at another time', () => {
    const healed = findHealableMembers(
      [group([member('work', 'gone')])],
      [ev('ev-x', 'work', 'Wochenplanung', '2026-08-10T15:00:00Z')],
      range,
      seriesId,
    );
    expect(healed).toEqual([]);
  });

  it('matches an all-day copy on its day', () => {
    const healed = findHealableMembers(
      [group([member('work', 'gone', '2026-08-10T00:00:00Z')])],
      [ev('new-a', 'work', 'Wochenplanung', '2026-08-10', true)],
      range,
      seriesId,
    );
    expect(healed).toHaveLength(1);
    expect(healed[0].new_event_id).toBe('new-a');
  });

  it('says nothing when there are no groups at all', () => {
    expect(findHealableMembers([], [], range, seriesId)).toEqual([]);
  });
});
