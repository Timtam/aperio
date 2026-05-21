/// <reference types="vitest" />
import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { useEventTabNavigation } from './useEventTabNavigation';
import type { CalendarEvent } from '../api/types';

function ev(id: string): CalendarEvent {
  return {
    id,
    calendar_id: 'c',
    title: id,
    description: null,
    location: null,
    start: '2026-05-21T10:00:00Z',
    end: '2026-05-21T11:00:00Z',
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
    created_at: '',
    updated_at: '',
    etag: null,
  };
}

interface HostState {
  dayIndex: number;
  buckets: { items: CalendarEvent[] }[];
}

function setup(buckets: CalendarEvent[][], startDay = 0) {
  const state: HostState = {
    dayIndex: startDay,
    buckets: buckets.map((items) => ({ items })),
  };
  const onDayChange = vi.fn();

  const result = renderHook(
    (props: { dayIndex: number }) =>
      useEventTabNavigation({
        buckets: state.buckets,
        dayIndex: props.dayIndex,
        setDayIndex: (next) => {
          state.dayIndex = next;
          result.rerender({ dayIndex: next });
        },
        onDayChange,
      }),
    { initialProps: { dayIndex: startDay } },
  );

  return { result, state, onDayChange };
}

describe('useEventTabNavigation', () => {
  it('first Tab from a populated day picks event 0 of that day', () => {
    const { result, state } = setup([[ev('a'), ev('b')], [ev('c')]], 0);
    act(() => {
      result.result.current.handleTab(false);
    });
    expect(result.result.current.eventIndex).toBe(0);
    expect(state.dayIndex).toBe(0);
  });

  it('Tab walks across day boundaries chronologically', () => {
    const { result, state, onDayChange } = setup(
      [[ev('a'), ev('b')], [ev('c')]],
      0,
    );
    // Day 0, event 0
    act(() => result.result.current.handleTab(false));
    // Day 0, event 1
    act(() => result.result.current.handleTab(false));
    expect(result.result.current.eventIndex).toBe(1);
    expect(state.dayIndex).toBe(0);
    // Day 1, event 0 — day change
    act(() => result.result.current.handleTab(false));
    expect(state.dayIndex).toBe(1);
    expect(result.result.current.eventIndex).toBe(0);
    expect(onDayChange).toHaveBeenCalledWith(1, expect.objectContaining({ id: 'c' }));
  });

  it('wraps from the last event back to the first', () => {
    const { result, state } = setup([[ev('a')], [ev('b')]], 1);
    act(() => result.result.current.handleTab(false)); // day 1 → ev0
    act(() => result.result.current.handleTab(false)); // wrap to day 0 ev0
    expect(state.dayIndex).toBe(0);
    expect(result.result.current.eventIndex).toBe(0);
  });

  it('Shift+Tab from a day cell jumps to the closest prior event', () => {
    const { result, state } = setup([[ev('a')], [], [ev('c'), ev('d')]], 1);
    act(() => result.result.current.handleTab(true));
    // From day 1 (empty), backward should land on day 0 ev 0.
    expect(state.dayIndex).toBe(0);
    expect(result.result.current.eventIndex).toBe(0);
  });

  it('Shift+Tab from a populated day cell skips the current day', () => {
    // The day cell logically sits before its own events in the tab
    // order, so stepping backward lands on the previous day's tail,
    // not on the current day's last event.
    const { result, state } = setup([[ev('a'), ev('b')], [ev('c'), ev('d')]], 1);
    act(() => result.result.current.handleTab(true));
    expect(state.dayIndex).toBe(0);
    expect(result.result.current.eventIndex).toBe(1);
  });

  it('Tab from a populated day cell dives into the current day', () => {
    const { result, state } = setup([[ev('a'), ev('b')], [ev('c'), ev('d')]], 1);
    act(() => result.result.current.handleTab(false));
    expect(state.dayIndex).toBe(1);
    expect(result.result.current.eventIndex).toBe(0);
  });

  it('skips empty days when walking forward', () => {
    const { result, state } = setup([[ev('a')], [], [ev('c')]], 0);
    act(() => result.result.current.handleTab(false)); // start at 0,0
    act(() => result.result.current.handleTab(false)); // → 2,0, day 1 skipped
    expect(state.dayIndex).toBe(2);
    expect(result.result.current.eventIndex).toBe(0);
  });

  it('does nothing when the period contains no events', () => {
    const { result, state } = setup([[], []], 0);
    const consumed = result.result.current.handleTab(false);
    expect(consumed).toBe(false);
    expect(result.result.current.eventIndex).toBeNull();
    expect(state.dayIndex).toBe(0);
  });

  it('is generic enough to walk a mixed event + task list', () => {
    // The bucket is whatever the caller wants — WeekView passes a
    // tagged union of events and tasks so SR Tab walks both kinds.
    // This test pins the contract: the hook indexes items by
    // position, the union is opaque to the navigation logic.
    type Item =
      | { kind: 'event'; title: string }
      | { kind: 'task'; title: string };
    const buckets: { items: Item[] }[] = [
      {
        items: [
          { kind: 'event', title: 'Standup' },
          { kind: 'task', title: 'Send report' },
        ],
      },
      { items: [{ kind: 'event', title: 'Lunch' }] },
    ];
    const onDayChange = vi.fn();
    let dayIndex = 0;
    const view = renderHook(
      () =>
        useEventTabNavigation<Item>({
          buckets,
          dayIndex,
          setDayIndex: (next) => {
            dayIndex = next;
            view.rerender();
          },
          onDayChange,
        }),
      {},
    );
    // First Tab → day 0, item 0 (event Standup).
    act(() => view.result.current.handleTab(false));
    expect(view.result.current.eventIndex).toBe(0);
    // Second Tab → day 0, item 1 (task Send report).
    act(() => view.result.current.handleTab(false));
    expect(view.result.current.eventIndex).toBe(1);
    // Third Tab → day 1, item 0 (event Lunch) — day change fires
    // with the task-flavoured item that just got skipped past.
    act(() => view.result.current.handleTab(false));
    expect(dayIndex).toBe(1);
    expect(view.result.current.eventIndex).toBe(0);
    expect(onDayChange).toHaveBeenLastCalledWith(
      1,
      expect.objectContaining({ kind: 'event', title: 'Lunch' }),
    );
  });
});
