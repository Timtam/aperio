import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';

import type { CalendarEvent } from '../api/types';

/**
 * Regression for the "count flashes on every page step" report: the SWR
 * cache was fine, but on a key change the hook's state still held the
 * PREVIOUS range's events for one render — the render before the effect
 * re-seeded it — so a paged-to week first showed (and a screen reader first
 * announced) whatever old-range events spanned into it, then the real set.
 * A cache hit must render the cached batch on the very first render after
 * the key changes; a cold key must render empty + loading, never stale rows.
 */

const getEventsMock = vi.hoisted(() =>
  vi.fn((req: { calendar_id: string; start: string; end: string }) =>
    Promise.resolve(FIXTURES[`${req.calendar_id}|${req.start}`] ?? []),
  ),
);
vi.mock('../api/client', () => ({ getEvents: getEventsMock }));
vi.mock('./calendarStoreContext', () => ({
  useCalendarStore: () => ({
    selectedCalendarIds: new Set(['a', 'b']),
    calendars: [],
    calendarsLoading: false,
  }),
}));
vi.mock('./dialogStateContext', () => ({
  useDialogState: () => ({ dataVersion: 0 }),
}));
vi.mock('./viewStateContext', () => ({
  useViewState: () => ({ focusedCalendarId: null, showCancelledEvents: true }),
}));

import { __resetEventsCacheForTests, useEvents } from './useEvents';

function event(id: string, start: string): CalendarEvent {
  return {
    id,
    calendar_id: 'a',
    title: id,
    start,
    end: start,
    all_day: false,
    cancelled: false,
  } as unknown as CalendarEvent;
}

const WEEK_1 = {
  start: new Date('2026-06-01T00:00:00.000Z'),
  end: new Date('2026-06-07T23:59:59.999Z'),
};
const WEEK_2 = {
  start: new Date('2026-06-08T00:00:00.000Z'),
  end: new Date('2026-06-14T23:59:59.999Z'),
};
const WEEK_3 = {
  start: new Date('2026-06-15T00:00:00.000Z'),
  end: new Date('2026-06-21T23:59:59.999Z'),
};

const FIXTURES: Record<string, CalendarEvent[]> = {
  [`a|${WEEK_1.start.toISOString()}`]: [
    event('w1-a1', '2026-06-02T09:00:00.000Z'),
    event('w1-a2', '2026-06-03T09:00:00.000Z'),
  ],
  [`b|${WEEK_1.start.toISOString()}`]: [event('w1-b1', '2026-06-05T09:00:00.000Z')],
  [`a|${WEEK_2.start.toISOString()}`]: [event('w2-a1', '2026-06-09T09:00:00.000Z')],
  [`b|${WEEK_2.start.toISOString()}`]: [],
};

beforeEach(() => {
  __resetEventsCacheForTests();
  getEventsMock.mockClear();
});

/** Every render's output, in order — `result.current` only shows the LAST
 *  render, and under act the effect's re-seed has already flushed by then.
 *  The screen reader hears the FIRST render after a key change (the commit
 *  React paints before running effects), so that is what the tests read. */
function renderLogged() {
  const log: { loading: boolean; ids: string[] }[] = [];
  const hook = renderHook(
    ({ range }: { range: { start: Date; end: Date } }) => {
      const r = useEvents(range);
      log.push({ loading: r.loading, ids: r.events.map((e) => e.id).sort() });
      return r;
    },
    { initialProps: { range: WEEK_1 } },
  );
  return { ...hook, log };
}

describe('useEvents key changes', () => {
  it('renders a warm range from the cache on the first render after paging', async () => {
    const { result, rerender, log } = renderLogged();
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.events.map((e) => e.id).sort()).toEqual([
      'w1-a1',
      'w1-a2',
      'w1-b1',
    ]);

    rerender({ range: WEEK_2 });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.events.map((e) => e.id)).toEqual(['w2-a1']);

    // Page BACK: the very first render after the key change must already be
    // week 1's cached batch — not week 2's single event under week 1's days.
    const before = log.length;
    rerender({ range: WEEK_1 });
    expect(log[before]).toEqual({
      loading: false,
      ids: ['w1-a1', 'w1-a2', 'w1-b1'],
    });
    // Let the background revalidation of the warm key land inside act.
    await act(async () => {});
  });

  it('renders a cold range as empty and loading, never the previous range', async () => {
    const { result, rerender } = renderHook(
      ({ range }: { range: { start: Date; end: Date } }) => useEvents(range),
      { initialProps: { range: WEEK_1 } },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.events).toHaveLength(3);

    rerender({ range: WEEK_3 });
    // Synchronously after the key change: no stale week-1 rows.
    expect(result.current.events).toHaveLength(0);
    expect(result.current.loading).toBe(true);
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.events).toHaveLength(0);
  });
});
