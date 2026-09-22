import { afterEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent } from '../api/types';

/**
 * "Delete this and all following" (decision 118).
 *
 * Cut at a later occurrence, the series ends just before it. Cut where nothing
 * comes before — the first occurrence — the series is deleted: truncating it
 * wrote a rule that ends before it starts, which the views hide and the
 * reminders fall back from, so the deleted series went on ringing.
 */

const { invokeMock, onFile } = vi.hoisted(() => {
  const onFile: { master: unknown } = { master: null };
  const invokeMock = vi.fn((command: string, _payload?: unknown) => {
    void _payload;
    if (command === 'get_event_by_id') return Promise.resolve(onFile.master);
    if (command === 'update_event') return Promise.resolve(onFile.master);
    return Promise.resolve(null);
  });
  return { invokeMock, onFile };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

/** A weekly Monday series, ten times, from 2026-08-03 09:00 UTC. */
const MASTER = {
  id: 'series-1',
  calendar_id: 'cal-work',
  title: 'Standup',
  description: null,
  location: null,
  start: '2026-08-03T09:00:00.000Z',
  end: '2026-08-03T09:15:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY;COUNT=10', exceptions: [], tzid: null },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

/** One occurrence of MASTER, as a view hands it over. */
const occurrenceAt = (iso: string) =>
  ({
    ...MASTER,
    id: `series-1@${iso}`,
    series_id: 'series-1',
    occurrence_start: iso,
    start: iso,
  }) as unknown as CalendarEvent;

const calls = (command: string) => invokeMock.mock.calls.filter((call) => call[0] === command);

afterEach(() => {
  invokeMock.mockClear();
  onFile.master = null;
});

describe('deleteThisAndFuture', () => {
  it('deletes the whole series at its first occurrence', async () => {
    onFile.master = MASTER;
    const { deleteThisAndFuture } = await import('./deleteSeriesFromOccurrence');
    const outcome = await deleteThisAndFuture(occurrenceAt(MASTER.start), MASTER.start, true);
    expect(outcome).toBe('deleted');
    expect(calls('update_event')).toHaveLength(0);
    expect(calls('delete_event').map((call) => call[1])).toEqual([
      { id: 'series-1', calendarId: 'cal-work', sendCancellations: true },
    ]);
  });

  it('ends the series before a later occurrence', async () => {
    onFile.master = MASTER;
    const { deleteThisAndFuture } = await import('./deleteSeriesFromOccurrence');
    const cut = '2026-08-24T09:00:00.000Z';
    const outcome = await deleteThisAndFuture(occurrenceAt(cut), cut, false);
    expect(outcome).toBe('truncated');
    expect(calls('delete_event')).toHaveLength(0);
    const sent = (calls('update_event')[0][1] as { event: CalendarEvent & { truncate_tail_overrides: boolean } })
      .event;
    expect(sent.id).toBe('series-1');
    expect(sent.recurrence?.rrule).toBe('FREQ=WEEKLY;UNTIL=20260824T085959Z');
    expect(sent.truncate_tail_overrides).toBe(true);
  });

  it('writes nothing when the cut cannot be read', async () => {
    // Written anyway, the unreadable instant became the series' end.
    onFile.master = MASTER;
    const { deleteThisAndFuture } = await import('./deleteSeriesFromOccurrence');
    await expect(
      deleteThisAndFuture(occurrenceAt(MASTER.start), 'not a date', false),
    ).rejects.toThrow();
    expect(calls('update_event')).toHaveLength(0);
    expect(calls('delete_event')).toHaveLength(0);
  });
});
