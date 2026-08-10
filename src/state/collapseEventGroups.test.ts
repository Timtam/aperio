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

describe('which member the folded row shows', () => {
  const LINK_GROUP = group('g-meeting', [
    ['acc::meetings', 'vc::1'],
    ['work', 'ev-a'],
  ]);

  it('never shows the read-only meetings copy when a real one is there', () => {
    // The two are at the identical instant — anything else marks the group
    // diverged and nothing folds — so the sort is a tie and the order is
    // whatever the calendar fan-out produced. Position must not decide which
    // row the user gets: the meetings copy has no editor, no delete, no move,
    // and on mobile is not even a button.
    const folded = collapseEventGroups(
      [
        row('vc::1', 'acc::meetings', '2026-08-10T08:00:00Z'),
        row('ev-a', 'work', '2026-08-10T08:00:00Z'),
      ],
      [LINK_GROUP],
      seriesId,
    );
    expect(folded).toHaveLength(1);
    expect(folded[0].event.id).toBe('ev-a');
    // The calendars it spans still lead with the row actually shown.
    expect(folded[0].calendarIds[0]).toBe('work');
  });

  it('shows the meetings copy when it is the only member in view', () => {
    // A meeting whose appointment is in a switched-off calendar is still a
    // meeting the user has, and hiding it would be the old filter's mistake.
    const folded = collapseEventGroups(
      [row('vc::1', 'acc::meetings', '2026-08-10T08:00:00Z')],
      [LINK_GROUP],
      seriesId,
    );
    expect(folded).toHaveLength(1);
    expect(folded[0].event.id).toBe('vc::1');
  });

  it('does not fold at all when the copies disagree about the time', () => {
    // The case the whole feature exists for: appointment moved, meeting not.
    // Both rows stay, and neither is quietly promoted over the other.
    const folded = collapseEventGroups(
      [
        row('vc::1', 'acc::meetings', '2026-08-10T08:00:00Z'),
        row('ev-a', 'work', '2026-08-10T09:00:00Z'),
      ],
      [LINK_GROUP],
      seriesId,
    );
    expect(folded.map((f) => f.event.id)).toEqual(['vc::1', 'ev-a']);
    expect(folded.every((f) => f.diverged)).toBe(true);
  });

  it('lets a caller answer for itself which rows are actionable', () => {
    const folded = collapseEventGroups(
      [
        row('ev-a', 'work', '2026-08-10T08:00:00Z'),
        row('ev-b', 'private', '2026-08-10T08:00:00Z'),
      ],
      [group('g2', [['work', 'ev-a'], ['private', 'ev-b']])],
      seriesId,
      (r) => r.calendar_id !== 'work',
    );
    expect(folded[0].event.id).toBe('ev-b');
  });
});

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

  it('shows the group through whichever copy is still switched on', () => {
    // Hiding a calendar must not hide the appointment. The row that stands for
    // a group is whichever member the view handed in first, so switching off
    // the calendar of the copy that used to represent it simply promotes the
    // next one — the group stays on screen as long as ANY of its calendars is
    // on, and the row that is drawn is one the view can actually open.
    const rows = collapseEventGroups(
      [
        row('ev-b', 'private', '2026-08-10T08:00:00Z'),
        row('ev-c', 'colleague', '2026-08-10T08:00:00Z'),
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
    expect(rows).toHaveLength(1);
    expect(rows[0].event.id).toBe('ev-b');
    // Still three copies, and the hidden calendar is still named.
    expect(rows[0].otherMembers).toBe(2);
    expect(rows[0].calendarIds).toEqual(['private', 'work', 'colleague']);
  });

  it('shows nothing when every calendar of the group is switched off', () => {
    // The other half of the same rule: a group is not a row of its own, it is
    // one of its members. With none of them in view there is nothing to draw —
    // and drawing a placeholder would put an appointment on a day the user has
    // deliberately emptied.
    const rows = collapseEventGroups(
      [],
      [
        group('g1', [
          ['work', 'ev-a'],
          ['private', 'ev-b'],
        ]),
      ],
      seriesId,
    );
    expect(rows).toEqual([]);
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
