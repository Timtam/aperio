import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { EventGroup } from '@aperio/shared';

import type { Calendar } from '../api/types';

/**
 * Carrying "this and all following" to a copy with nothing before its cut
 * point (decision 118).
 *
 * Cut like the others, such a copy was left with a head that ends before it
 * starts — hidden from the views, still ringing. It is rewritten in place
 * instead and, like a single event, joins the new rows. And when the anchor
 * itself was rewritten in place, it is still in the group of heads: it leaves
 * that group first — only if it is still there — or the new rows would be
 * pulled into it.
 */

const { invokeMock, groupOfAnchor, copyOnFile } = vi.hoisted(() => {
  const groupOfAnchor: { current: unknown[] } = { current: [] };
  /** The copy as `get_event_by_id` answers; COPY unless a test says otherwise. */
  const copyOnFile: { current: unknown } = { current: null };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'get_event_by_id') {
      return Promise.resolve(copyOnFile.current ?? COPY);
    }
    if (command === 'get_events') {
      return Promise.resolve([]);
    }
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'event_groups_for_events') {
      return Promise.resolve(groupOfAnchor.current);
    }
    if (command === 'group_events') {
      return Promise.resolve({ id: 'g-new', members: [] });
    }
    return Promise.resolve(null);
  });
  return { invokeMock, groupOfAnchor, copyOnFile };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS = [
  { id: 'work', name: 'Arbeit', read_only: false, account_id: 'acc-a' },
  { id: 'private', name: 'Privat', read_only: false, account_id: 'acc-b' },
] as unknown as Calendar[];
const STORE = { calendars: CALENDARS, colorLabels: [], selectedCalendarIds: new Set(['work']) };
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

const CUT = '2026-08-24T08:00:00.000Z';

/** The private copy: a weekly series that starts on the day of the cut. */
const COPY = {
  id: 'ev-b',
  calendar_id: 'private',
  title: 'Wochenplanung',
  description: null,
  location: 'Raum 3',
  start: CUT,
  end: '2026-08-24T09:00:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY;COUNT=10', exceptions: [], tzid: null },
  color_label: 'blue',
  reminders: [],
  attendees: [],
};

const GROUP: EventGroup = {
  id: 'g1',
  created_at: '2026-08-09T12:00:00Z',
  updated_at: '2026-08-09T12:00:00Z',
  members: [
    {
      calendar_id: 'work',
      event_id: 'ev-a',
      title: 'Wochenplanung',
      starts_at: CUT,
      added_at: '2026-08-09T12:00:00Z',
    },
    {
      calendar_id: 'private',
      event_id: 'ev-b',
      title: 'Wochenplanung',
      starts_at: CUT,
      added_at: '2026-08-09T12:00:01Z',
    },
  ],
};

const STOOD = {
  title: 'Wochenplanung',
  start: CUT,
  end: '2026-08-24T09:00:00.000Z',
  all_day: false,
  location: 'Raum 3',
  description: null,
};
const MOVED = { ...STOOD, start: '2026-08-24T09:00:00.000Z', end: '2026-08-24T10:00:00.000Z' };

/** The anchor, rewritten in place: its successor is itself. */
const ANCHOR = { calendar_id: 'work', event_id: 'ev-a' };
const SUCCESSOR = { ...ANCHOR, title: 'Wochenplanung', starts_at: MOVED.start };

const calls = (command: string) => invokeMock.mock.calls.filter((call) => call[0] === command);

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  groupOfAnchor.current = [];
  copyOnFile.current = null;
});

async function carry() {
  const { EventGroupCarryDialog } = await import('./EventGroupCarryDialog');
  render(
    <EventGroupCarryDialog
      isOpen
      onClose={() => {}}
      group={GROUP}
      anchor={ANCHOR}
      before={STOOD}
      after={MOVED}
      scope="future"
      occurrence={CUT}
      successor={SUCCESSOR}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: /mitziehen|carry over/i }));
  await waitFor(() => expect(calls('group_events')).toHaveLength(1));
}

describe('EventGroupCarryDialog → "this and all following" to a copy with no head', () => {
  it('rewrites the copy in place and ties it to the anchor in a new group', async () => {
    groupOfAnchor.current = [GROUP];
    await carry();

    expect(calls('create_event')).toHaveLength(0);
    const sent = (calls('update_event')[0][1] as { event: typeof COPY & { send_invitations: boolean } })
      .event;
    expect(sent.id).toBe('ev-b');
    expect(sent.start).toBe('2026-08-24T09:00:00.000Z');
    expect(sent.recurrence.rrule).toBe('FREQ=WEEKLY;COUNT=10');
    expect(sent.send_invitations).toBe(false);
    // The copy leaves the heads' group, and so does the anchor that is still
    // in it — as bookkeeping, recording no "not the same appointment".
    expect(calls('ungroup_event').map((call) => call[1])).toEqual([
      { calendarId: 'private', eventId: 'ev-b', bookkeeping: true },
      { calendarId: 'work', eventId: 'ev-a', bookkeeping: true },
    ]);
    const members = (calls('group_events')[0][1] as { members: { event_id: string }[] }).members;
    expect(members.map((m) => m.event_id)).toEqual(['ev-a', 'ev-b']);
  });

  it('rewrites a copy whose earlier occurrences were all deleted from its cut on', async () => {
    // Two Mondays before the cut, both deleted: nothing of it is shown before,
    // so it is written whole — from the cut, with what is left of its COUNT.
    groupOfAnchor.current = [GROUP];
    copyOnFile.current = {
      ...COPY,
      start: '2026-08-10T08:00:00.000Z',
      end: '2026-08-10T09:00:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;COUNT=10',
        exceptions: ['2026-08-10T08:00:00.000Z', '2026-08-17T08:00:00.000Z'],
        tzid: null,
      },
    };
    await carry();

    expect(calls('create_event')).toHaveLength(0);
    const sent = (calls('update_event')[0][1] as { event: typeof COPY }).event;
    expect(sent.id).toBe('ev-b');
    expect(sent.start).toBe('2026-08-24T09:00:00.000Z');
    expect(sent.recurrence).toEqual({ rrule: 'FREQ=WEEKLY;COUNT=8', exceptions: [], tzid: null });
  });

  it("moves the copy's exceptions with its new time", async () => {
    // Left at 08:00, the excluded Monday came back at 09:00.
    groupOfAnchor.current = [GROUP];
    copyOnFile.current = {
      ...COPY,
      recurrence: { ...COPY.recurrence, exceptions: ['2026-08-31T08:00:00.000Z'] },
    };
    await carry();

    const sent = (calls('update_event')[0][1] as { event: typeof COPY }).event;
    expect(sent.recurrence.exceptions).toEqual(['2026-08-31T09:00:00.000Z']);
  });

  it('leaves the anchor where it is when it is no longer in the carried group', async () => {
    // A retry: the anchor is already in the new group, and taking it out of
    // that one would undo it.
    groupOfAnchor.current = [{ ...GROUP, id: 'g-new' }];
    await carry();

    expect(calls('ungroup_event').map((call) => call[1])).toEqual([
      { calendarId: 'private', eventId: 'ev-b', bookkeeping: true },
    ]);
  });
});
