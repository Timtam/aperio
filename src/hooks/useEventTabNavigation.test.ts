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
  buckets: { events: CalendarEvent[] }[];
}

function setup(buckets: CalendarEvent[][], startDay = 0) {
  const state: HostState = {
    dayIndex: startDay,
    buckets: buckets.map((events) => ({ events })),
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
});
