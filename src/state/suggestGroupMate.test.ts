import { describe, expect, it } from 'vitest';

import { suggestGroupMate } from '@aperio/shared';

const ev = (
  id: string,
  calendar_id: string,
  title: string,
  start: string,
  all_day = false,
) => ({ id, calendar_id, title, start, all_day });

const anchor = ev('ev-a', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z');

describe('recognising a copy', () => {
  it('finds the same appointment in another calendar', () => {
    const found = suggestGroupMate(anchor, [
      ev('ev-x', 'private', 'Standup', '2026-08-10T08:00:00Z'),
      ev('ev-b', 'private', '  wochenplanung ', '2026-08-10T08:00:00Z'),
    ]);
    expect(found?.id).toBe('ev-b');
  });

  it('refuses a meeting that merely overlaps', () => {
    // "Overlapping" would happily offer the meeting before this one, which is
    // the wrong answer in the most ordinary calendar there is.
    expect(
      suggestGroupMate(anchor, [
        ev('ev-b', 'private', 'Wochenplanung', '2026-08-10T07:30:00Z'),
      ]),
    ).toBeNull();
  });

  it('refuses a second row in the same calendar', () => {
    // Two rows in one calendar are a duplicate to clean up, not an
    // appointment that lives in several places.
    expect(
      suggestGroupMate(anchor, [
        ev('ev-b', 'work', 'Wochenplanung', '2026-08-10T08:00:00Z'),
      ]),
    ).toBeNull();
  });

  it('refuses a different name at the same time', () => {
    expect(
      suggestGroupMate(anchor, [
        ev('ev-b', 'private', 'Zahnarzt', '2026-08-10T08:00:00Z'),
      ]),
    ).toBeNull();
  });

  it('says nothing about a nameless event', () => {
    // Every untitled row would otherwise look like a copy of every other one.
    expect(
      suggestGroupMate({ ...anchor, title: '   ' }, [
        ev('ev-b', 'private', '', '2026-08-10T08:00:00Z'),
      ]),
    ).toBeNull();
  });

  it('matches all-day events on the day, not the instant', () => {
    const allDayAnchor = ev('ev-a', 'work', 'Urlaub', '2026-08-10', true);
    const found = suggestGroupMate(allDayAnchor, [
      ev('ev-b', 'private', 'Urlaub', '2026-08-10T00:00:00Z', true),
    ]);
    expect(found?.id).toBe('ev-b');
  });

  it('takes the first of several, in the order the user sees', () => {
    const found = suggestGroupMate(anchor, [
      ev('ev-b', 'private', 'Wochenplanung', '2026-08-10T08:00:00Z'),
      ev('ev-c', 'colleague', 'Wochenplanung', '2026-08-10T08:00:00Z'),
    ]);
    expect(found?.id).toBe('ev-b');
  });
});
