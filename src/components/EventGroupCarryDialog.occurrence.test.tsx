import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { EventGroup } from '@aperio/shared';

import type { Calendar } from '../api/types';

/**
 * Carrying "only this occurrence" to a copy: its standalone is created first,
 * then the occurrence is left out of its series (decision 277).
 *
 * A create that fails leaves the copy whole, so it is "not changed". A skip
 * that fails after the standalone landed is never that: a busy provider that
 * kept the occurrence is said as the carve-out's own sentence, any other
 * failure as a later step's, and either way the copy counts as written and is
 * not offered again — a retry would make a second standalone.
 */

const { invokeMock, createFails, skipFails, announced } = vi.hoisted(() => {
  /** What creating the copy's standalone fails with, when a test says so. */
  const createFails: { current: unknown } = { current: null };
  /** What leaving the occurrence out of the copy's series fails with. */
  const skipFails: { current: unknown } = { current: null };
  /** What the dialog announced, in order. */
  const announced: string[] = [];
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'get_event_by_id') return Promise.resolve(COPY);
    if (command === 'create_event') {
      if (createFails.current != null) return Promise.reject(createFails.current);
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'ev-b-single' });
    }
    if (command === 'add_event_exdate') {
      if (skipFails.current != null) return Promise.reject(skipFails.current);
      return Promise.resolve(null);
    }
    if (command === 'event_groups_for_events') return Promise.resolve([]);
    if (command === 'group_events') return Promise.resolve({ id: 'g-new', members: [] });
    return Promise.resolve(null);
  });
  return { invokeMock, createFails, skipFails, announced };
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
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));

const OCCURRENCE = '2026-08-24T08:00:00.000Z';

/** The private copy: a weekly series with an occurrence at OCCURRENCE. */
const COPY = {
  id: 'ev-b',
  calendar_id: 'private',
  title: 'Wochenplanung',
  description: null,
  location: 'Raum 3',
  start: '2026-08-17T08:00:00.000Z',
  end: '2026-08-17T09:00:00.000Z',
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
      starts_at: '2026-08-17T08:00:00.000Z',
      added_at: '2026-08-09T12:00:00Z',
    },
    {
      calendar_id: 'private',
      event_id: 'ev-b',
      title: 'Wochenplanung',
      starts_at: '2026-08-17T08:00:00.000Z',
      added_at: '2026-08-09T12:00:01Z',
    },
  ],
};

const STOOD = {
  title: 'Wochenplanung',
  start: OCCURRENCE,
  end: '2026-08-24T09:00:00.000Z',
  all_day: false,
  location: 'Raum 3',
  description: null,
};
const MOVED = { ...STOOD, start: '2026-08-24T09:00:00.000Z', end: '2026-08-24T10:00:00.000Z' };

/** The anchor's occurrence, carved out of its own series as a standalone. */
const SUCCESSOR = {
  calendar_id: 'work',
  event_id: 'ev-a-single',
  title: 'Wochenplanung',
  starts_at: MOVED.start,
};

const calls = (command: string) => invokeMock.mock.calls.filter((call) => call[0] === command);
const BUSY = { code: 'forbidden', message: 'provider-busy: http-429' };

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  createFails.current = null;
  skipFails.current = null;
  announced.length = 0;
});

async function carry() {
  const { EventGroupCarryDialog } = await import('./EventGroupCarryDialog');
  render(
    <EventGroupCarryDialog
      isOpen
      onClose={() => {}}
      group={GROUP}
      anchor={{ calendar_id: 'work', event_id: 'ev-a' }}
      before={STOOD}
      after={MOVED}
      scope="occurrence"
      occurrence={OCCURRENCE}
      successor={SUCCESSOR}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: /mitziehen|carry over/i }));
}

describe('EventGroupCarryDialog → "only this occurrence" to a copy', () => {
  it('creates the standalone first, then leaves the occurrence out of the series', async () => {
    await carry();
    await waitFor(() => expect(calls('group_events')).toHaveLength(1));
    const order = invokeMock.mock.calls
      .map((call) => call[0])
      .filter((command) => command === 'create_event' || command === 'add_event_exdate');
    expect(order).toEqual(['create_event', 'add_event_exdate']);
  });

  it('says a copy "not changed" when its standalone could not be created, and skips nothing', async () => {
    createFails.current = BUSY;
    await carry();
    expect(await screen.findByText(/Nicht geändert: Privat|Not changed: Privat/)).toBeTruthy();
    expect(calls('add_event_exdate')).toHaveLength(0);
  });

  it('says a standalone whose answer was lost as one that may stand, and offers it not again', async () => {
    // Review round 17 of #128: "not changed", and trying again made a second
    // standalone.
    createFails.current = { code: 'network', message: 'connection reset' };
    await carry();
    const mayStand = /In Privat steht der Termin am .* möglicherweise doppelt|In Privat, the appointment may now show twice/;
    await waitFor(() => expect(announced.some((line) => mayStand.test(line))).toBe(true));
    expect(screen.queryByText(/Nicht geändert|Not changed/)).toBeNull();
    expect(screen.queryByRole('button', { name: /erneut|again/i })).toBeNull();
    expect(calls('add_event_exdate')).toHaveLength(0);
  });

  it("says the copy's kept occurrence when a busy provider refused the skip after its standalone (277)", async () => {
    skipFails.current = BUSY;
    await carry();
    const kept = /Termin der Serie in „Privat“ ließ sich nicht auslassen|series' occurrence in "Privat" could not be left out/;
    await waitFor(() => expect(announced.some((line) => kept.test(line))).toBe(true));
    // Written: its standalone joins the new rows, and nothing offers it again.
    await waitFor(() => expect(calls('group_events')).toHaveLength(1));
    const members = (calls('group_events')[0][1] as { members: { event_id: string }[] }).members;
    expect(members.map((m) => m.event_id)).toEqual(['ev-a-single', 'ev-b-single']);
    expect(screen.getByText(/Eine Kopie aktualisiert|One copy updated/)).toBeTruthy();
    expect(screen.queryByText(/Nicht geändert|Not changed/)).toBeNull();
    expect(screen.queryByRole('button', { name: /erneut|again/i })).toBeNull();
    // Said once, the count and the kept occurrence together.
    const said = announced.filter((line) => kept.test(line));
    expect(said).toHaveLength(1);
    expect(said[0]).toMatch(/Eine Kopie aktualisiert|One copy updated/);
    expect(said[0]).not.toMatch(/nichts geändert|nothing was changed/i);
  });

  it('says a later failure of the skip as one, never "not changed" (277)', async () => {
    // A refusal whose own sentence says nothing was changed: after the
    // standalone landed, it must not reach the copy's line as it is.
    skipFails.current = { code: 'forbidden', message: 'server-refused: 403' };
    await carry();
    await waitFor(() => expect(calls('group_events')).toHaveLength(1));
    expect(screen.getByText(/Eine Kopie aktualisiert|One copy updated/)).toBeTruthy();
    expect(screen.queryByText(/Nicht geändert|Not changed/)).toBeNull();
    const said = announced.find((line) => /Privat:/.test(line));
    expect(said).toBeDefined();
    expect(said).not.toMatch(/nichts geändert|nothing was changed/i);
  });
});
