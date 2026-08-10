import { describe, expect, it } from 'vitest';

import { collapseEventGroups, type EventGroup } from '@aperio/shared';

interface Row {
  id: string;
  calendar_id: string;
  start: string;
  all_day?: boolean;
  title: string;
}

const row = (id: string, calendar: string, start: string, title = 'Wochenplanung'): Row => ({
  id,
  calendar_id: calendar,
  start,
  title,
});

const group = (id: string, members: [string, string][]): EventGroup => ({
  id,
  created_at: '2026-08-09T12:00:00Z',
  updated_at: '2026-08-09T12:00:00Z',
  members: members.map(([calendar, event]) => ({
    calendar_id: calendar,
    event_id: event,
    title: 'Wochenplanung',
    starts_at: '2026-08-10T08:00:00Z',
    added_at: '2026-08-09T12:00:00Z',
  })),
});

const seriesId = (r: Row) => r.id;

describe('collapsing groups into one row', () => {
  it('keeps the first member and drops the rest, without reordering', () => {
    const events = [
      row('ev-early', 'work', '2026-08-10T07:00:00Z', 'Standup'),
      row('ev-a', 'work', '2026-08-10T08:00:00Z'),
      row('ev-b', 'private', '2026-08-10T08:00:00Z'),
      row('ev-late', 'work', '2026-08-10T09:00:00Z', 'Retro'),
    ];
    const rows = collapseEventGroups(
      events,
      [group('g1', [['work', 'ev-a'], ['private', 'ev-b']])],
      seriesId,
    );

    expect(rows.map((r) => r.event.id)).toEqual(['ev-early', 'ev-a', 'ev-late']);
    const folded = rows[1];
    expect(folded.otherMembers).toBe(1);
    expect(folded.calendarIds).toEqual(['work', 'private']);
    expect(folded.diverged).toBe(false);
  });

  it('counts the members the view cannot see', () => {
    // Only one copy is in a switched-on calendar. The row still says the
    // appointment exists three times — that is what makes the count match
    // what the user knows they keep.
    const rows = collapseEventGroups(
      [row('ev-a', 'work', '2026-08-10T08:00:00Z')],
      [
        group('g1', [
          ['work', 'ev-a'],
          ['private', 'ev-b'],
          ['colleague', 'ev-c'],
        ]),
      ],
      seriesId,
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].otherMembers).toBe(2);
    expect(rows[0].calendarIds).toEqual(['work', 'private', 'colleague']);
  });

  it('folds two spellings of the SAME instant', () => {
    // The bug an adversarial review found: `expandAll` rewrites a recurring
    // occurrence through toISOString() while a one-off keeps the backend's own
    // serialisation. Compared as strings those look like two different times,
    // so a series grouped with a single event never folded — and BOTH copies
    // announced "which is now at a different time", every day.
    const rows = collapseEventGroups(
      [
        row('ev-a', 'work', '2026-08-10T08:00:00.000Z'),
        row('ev-b', 'private', '2026-08-10T08:00:00Z'),
      ],
      [group('g1', [['work', 'ev-a'], ['private', 'ev-b']])],
      seriesId,
    );

    expect(rows.map((r) => r.event.id)).toEqual(['ev-a']);
    expect(rows[0].diverged).toBe(false);
  });

  it('refuses to fold a group whose copies have drifted apart', () => {
    // One copy was moved and the others were not, so the group is a claim
    // that has stopped being true. Folding would hide exactly the problem.
    const rows = collapseEventGroups(
      [
        row('ev-a', 'work', '2026-08-10T08:00:00Z'),
        row('ev-b', 'private', '2026-08-10T10:00:00Z'),
      ],
      [group('g1', [['work', 'ev-a'], ['private', 'ev-b']])],
      seriesId,
    );

    expect(rows.map((r) => r.event.id)).toEqual(['ev-a', 'ev-b']);
    expect(rows.every((r) => r.diverged)).toBe(true);
  });

  it('gives the widget one line for an appointment, not four', () => {
    // The widget has less room than any view in the app, so a copy it need
    // not show is a line it can give to the next real thing. Same rule, same
    // helper — the shape is what differs.
    const rows = collapseEventGroups(
      [
        row('ev-a', 'work', '2026-08-10T08:00:00Z'),
        row('ev-b', 'private', '2026-08-10T08:00:00Z'),
        row('ev-c', 'colleague', '2026-08-10T08:00:00Z'),
        row('ev-next', 'work', '2026-08-10T09:00:00Z', 'Zahnarzt'),
      ],
      [
        group('g1', [
          ['work', 'ev-a'],
          ['private', 'ev-b'],
          ['colleague', 'ev-c'],
        ]),
      ],
      seriesId,
    );
    expect(rows.map((r) => r.event.id)).toEqual(['ev-a', 'ev-next']);
  });

  it('leaves an ungrouped day exactly as it was', () => {
    const events = [
      row('ev-a', 'work', '2026-08-10T08:00:00Z'),
      row('ev-b', 'private', '2026-08-10T09:00:00Z'),
    ];
    const rows = collapseEventGroups(events, [], seriesId);
    expect(rows.map((r) => r.event)).toEqual(events);
    expect(rows.every((r) => r.otherMembers === 0 && !r.group)).toBe(true);
  });

  it('folds a recurring group once per day, called a day at a time', () => {
    // A recurring appointment renders one row per day. Called per day — which
    // is the documented contract, and what every view does anyway — each day
    // folds on its own and the series' own different days are not mistaken
    // for copies that have drifted apart.
    const master = (r: Row) => r.id.split('::rid::')[0];
    const groups = [group('g1', [['work', 'ev-a'], ['private', 'ev-b']])];
    const monday = collapseEventGroups(
      [
        row('ev-a::rid::2026-08-10', 'work', '2026-08-10T08:00:00Z'),
        row('ev-b::rid::2026-08-10', 'private', '2026-08-10T08:00:00Z'),
      ],
      groups,
      master,
    );
    const tuesday = collapseEventGroups(
      [
        row('ev-a::rid::2026-08-11', 'work', '2026-08-11T08:00:00Z'),
        row('ev-b::rid::2026-08-11', 'private', '2026-08-11T08:00:00Z'),
      ],
      groups,
      master,
    );

    expect(monday.map((r) => r.event.id)).toEqual(['ev-a::rid::2026-08-10']);
    expect(tuesday.map((r) => r.event.id)).toEqual(['ev-a::rid::2026-08-11']);
    expect(monday[0].diverged).toBe(false);
    expect(tuesday[0].otherMembers).toBe(1);
  });
});
