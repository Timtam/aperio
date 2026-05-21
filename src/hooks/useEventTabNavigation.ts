import { useCallback, useEffect, useMemo, useRef } from 'react';

import type { CalendarEvent } from '../api/types';

/**
 * Outlook-style Tab navigation across the events of the currently
 * visible period.
 *
 * The host view (week or month) keeps two levels of focus on top of
 * the existing `aria-activedescendant`: the focused day (`dayIndex`,
 * managed by the view) and the optionally focused event inside that
 * day (`eventIndex`, managed by this hook).
 *
 * Tab moves to the next event in chronological order, *crossing day
 * boundaries when necessary*. Shift+Tab goes back. When the day
 * changes the host's anchor is moved so that the visual selection on
 * the day cell follows along, and an `announceDayChange` callback
 * fires so the host can read out the new day name through the
 * existing live announcer.
 *
 * The first Tab from a day cell starts at the first event of the
 * focused day, falling forward to the next day with events when the
 * current day is empty (Shift+Tab walks backwards). Once the view
 * is past the last / first event we wrap to the other end — staying
 * inside the period keeps the cycle finite and predictable for
 * keyboard-only users.
 */
export interface DayEventsBucket<T = CalendarEvent> {
  /**
   * Focusable items for one day, in the order Tab should walk them.
   * The hook is agnostic about whether these are pure events or a
   * merged list — WeekView, for example, puts events *and* timed
   * tasks here so Shift+Tab walks both kinds. MonthView keeps the
   * default CalendarEvent shape.
   */
  items: T[];
}

export interface UseEventTabNavigationOptions<T = CalendarEvent> {
  /** Ordered list of days currently visible. */
  buckets: DayEventsBucket<T>[];
  /** Index into `buckets` of the day cell currently selected by the user. */
  dayIndex: number;
  /** Setter for the host's day index — called when Tab crosses days. */
  setDayIndex: (next: number) => void;
  /**
   * Called after a Tab moves to a different day. Receives the bucket
   * index *and* the item the user just stepped onto, so the host can
   * read out something like "Wednesday, Aug 21: Team meeting".
   */
  onDayChange?: (newDayIndex: number, item: T) => void;
}

export interface UseEventTabNavigationResult {
  /** Currently focused event in the focused day, or null = day cell. */
  eventIndex: number | null;
  /** Manually clear (e.g. when the user presses Escape). */
  clear: () => void;
  /**
   * Handle a Tab / Shift+Tab key press. Returns `true` if the press
   * was consumed (host must call `preventDefault`), `false` otherwise.
   */
  handleTab: (shift: boolean) => boolean;
  /**
   * Move directly to the n-th event of the currently focused day,
   * used by click handlers or programmatic focus.
   */
  setEventIndex: (next: number | null) => void;
}

import { useState } from 'react';

export function useEventTabNavigation<T = CalendarEvent>(
  opts: UseEventTabNavigationOptions<T>,
): UseEventTabNavigationResult {
  const { buckets, dayIndex, setDayIndex, onDayChange } = opts;

  // Total item count across the visible period — drives wrap-around.
  const total = useMemo(
    () => buckets.reduce((acc, b) => acc + b.items.length, 0),
    [buckets],
  );

  const [eventIndex, setEventIndex] = useState<number | null>(null);

  // The host resets eventIndex to null whenever the day changes via
  // arrow keys, but Tab needs to *change* the day AND keep an event
  // selected. We mark "this day change is mine, don't reset" through
  // a ref so the day-effect runs without clobbering the new event.
  const keepEventRef = useRef(false);
  useEffect(() => {
    if (keepEventRef.current) {
      keepEventRef.current = false;
      return;
    }
    setEventIndex(null);
  }, [dayIndex]);

  // Clamp when the focused day's event list shrinks (e.g. delete).
  const dayEventCount = buckets[dayIndex]?.items.length ?? 0;
  useEffect(() => {
    if (eventIndex !== null && eventIndex >= dayEventCount) {
      setEventIndex(dayEventCount > 0 ? dayEventCount - 1 : null);
    }
  }, [dayEventCount, eventIndex]);

  // Translate a (dayIdx, evIdx) pair into a single flat index for
  // chronological cycling. The reverse mapping walks the buckets again
  // because a hash lookup would not buy us anything for the 7-42 days
  // a grid ever shows.
  const toFlat = useCallback(
    (dIdx: number, eIdx: number): number => {
      let acc = 0;
      for (let i = 0; i < dIdx; i++) acc += buckets[i]?.items.length ?? 0;
      return acc + eIdx;
    },
    [buckets],
  );

  const fromFlat = useCallback(
    (flat: number): { dayIdx: number; evIdx: number } => {
      let acc = 0;
      for (let i = 0; i < buckets.length; i++) {
        const len = buckets[i]?.items.length ?? 0;
        if (flat < acc + len) return { dayIdx: i, evIdx: flat - acc };
        acc += len;
      }
      return { dayIdx: 0, evIdx: 0 };
    },
    [buckets],
  );

  // First Tab from a day cell: pick a sensible entry point.
  //
  // Tab forward includes the current day — the cell logically sits at
  // the *start* of the day in the tab order, so the first Tab moves
  // into that day's first event. Shift+Tab moves backwards by one
  // step, which lands on the *previous* day's last event (the cell
  // itself is "before" its own events, so the step before it is the
  // tail of the prior day). Empty days are skipped in both
  // directions so navigation never dead-ends.
  const firstEventForward = useCallback(
    (
      startDay: number,
      includeStart: boolean,
    ): { dayIdx: number; evIdx: number } | null => {
      const offset = includeStart ? 0 : 1;
      for (let i = offset; i < buckets.length; i++) {
        const idx = (startDay + i) % buckets.length;
        if ((buckets[idx]?.items.length ?? 0) > 0) {
          return { dayIdx: idx, evIdx: 0 };
        }
      }
      return null;
    },
    [buckets],
  );

  const firstEventBackward = useCallback(
    (
      startDay: number,
      includeStart: boolean,
    ): { dayIdx: number; evIdx: number } | null => {
      const offset = includeStart ? 0 : 1;
      for (let i = offset; i < buckets.length; i++) {
        const idx = (startDay - i + buckets.length) % buckets.length;
        const len = buckets[idx]?.items.length ?? 0;
        if (len > 0) return { dayIdx: idx, evIdx: len - 1 };
      }
      return null;
    },
    [buckets],
  );

  const apply = useCallback(
    (next: { dayIdx: number; evIdx: number }) => {
      const dayChanged = next.dayIdx !== dayIndex;
      if (dayChanged) {
        keepEventRef.current = true;
        setDayIndex(next.dayIdx);
        const item = buckets[next.dayIdx]?.items[next.evIdx];
        if (item && onDayChange) onDayChange(next.dayIdx, item);
      }
      setEventIndex(next.evIdx);
    },
    [dayIndex, setDayIndex, buckets, onDayChange],
  );

  const handleTab = useCallback(
    (shift: boolean): boolean => {
      if (total === 0) return false;

      // No event focused yet — pick the entry point. Forward Tab
      // dives into the current day's first event (the day cell sits
      // "before" its events in the linear tab order). Shift+Tab walks
      // the other way and lands on the previous day's last event, so
      // the cell behaves like a fence post between yesterday's tail
      // and today's head.
      if (eventIndex === null) {
        const next = shift
          ? firstEventBackward(dayIndex, /* includeStart */ false)
          : firstEventForward(dayIndex, /* includeStart */ true);
        if (next) apply(next);
        return true;
      }

      const current = toFlat(dayIndex, eventIndex);
      const nextFlat = shift
        ? (current - 1 + total) % total
        : (current + 1) % total;
      apply(fromFlat(nextFlat));
      return true;
    },
    [
      total,
      eventIndex,
      dayIndex,
      firstEventForward,
      firstEventBackward,
      toFlat,
      fromFlat,
      apply,
    ],
  );

  const clear = useCallback(() => setEventIndex(null), []);

  return { eventIndex, clear, handleTab, setEventIndex };
}
