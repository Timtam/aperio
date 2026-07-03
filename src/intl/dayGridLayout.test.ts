import { describe, expect, it } from 'vitest';

import {
  dropMinuteInWindow,
  eventBlockFactor,
  layoutDayColumn,
  minutesFromMidnight,
  MINUTES_PER_DAY,
  type TimedSpan,
} from '@aperio/shared';

// Helper: minutes for HH:MM.
const at = (h: number, m = 0): number => h * 60 + m;
const span = (startMin: number, endMin: number): TimedSpan => ({
  startMin,
  endMin,
});

describe('layoutDayColumn', () => {
  it('positions a single event by start + duration (full width)', () => {
    const [p] = layoutDayColumn([span(at(9), at(10, 30))]);
    expect(p.topFraction).toBeCloseTo(at(9) / MINUTES_PER_DAY);
    expect(p.heightFraction).toBeCloseTo(90 / MINUTES_PER_DAY);
    expect(p.columnIndex).toBe(0);
    expect(p.columnCount).toBe(1);
  });

  it('keeps non-overlapping events full width', () => {
    const out = layoutDayColumn([span(at(9), at(10)), span(at(11), at(12))]);
    expect(out.every((p) => p.columnCount === 1)).toBe(true);
  });

  it('treats back-to-back (touching) events as NON-overlapping', () => {
    // 9–10 and 10–11 share an edge but must not overlap → both full width.
    const out = layoutDayColumn([span(at(9), at(10)), span(at(10), at(11))]);
    expect(out.map((p) => p.columnCount)).toEqual([1, 1]);
  });

  it('splits two overlapping events into two columns', () => {
    const out = layoutDayColumn([span(at(9), at(10, 30)), span(at(10), at(11))]);
    expect(out.map((p) => p.columnCount)).toEqual([2, 2]);
    expect(out.map((p) => p.columnIndex).sort()).toEqual([0, 1]);
  });

  it('gives a three-way overlap three columns', () => {
    const out = layoutDayColumn([
      span(at(9), at(12)),
      span(at(9, 30), at(11)),
      span(at(10), at(10, 30)),
    ]);
    expect(out.every((p) => p.columnCount === 3)).toBe(true);
    expect(out.map((p) => p.columnIndex).sort()).toEqual([0, 1, 2]);
  });

  it('chains a transitive (staircase) overlap into one cluster', () => {
    // A 9–10, B 9:30–10:30, C 10:15–11. A∩B and B∩C overlap but A∩C do not;
    // they still form ONE cluster, so all three report columnCount 3... no —
    // A and C don't overlap, so C can REUSE A's column. Cluster width is the
    // peak concurrency (2 at any instant), so columnCount is 2.
    const out = layoutDayColumn([
      span(at(9), at(10)),
      span(at(9, 30), at(10, 30)),
      span(at(10, 15), at(11)),
    ]);
    // Peak concurrency is 2 (B overlaps both A and C, but A and C are disjoint).
    expect(out.map((p) => p.columnCount)).toEqual([2, 2, 2]);
    // A in col 0; B forced to col 1; C reuses col 0 (A has ended by 10:15).
    expect(out[0].columnIndex).toBe(0);
    expect(out[1].columnIndex).toBe(1);
    expect(out[2].columnIndex).toBe(0);
  });

  it('preserves INPUT order in the output array', () => {
    // Pass them out of chronological order; result[i] must position input[i].
    const out = layoutDayColumn([span(at(14), at(15)), span(at(9), at(10))]);
    expect(out[0].topFraction).toBeCloseTo(at(14) / MINUTES_PER_DAY);
    expect(out[1].topFraction).toBeCloseTo(at(9) / MINUTES_PER_DAY);
  });

  it('gives zero-duration points (timed tasks) their own columns when coincident', () => {
    const out = layoutDayColumn([span(at(9), at(9)), span(at(9), at(9))]);
    expect(out.every((p) => p.columnCount === 2)).toBe(true);
    expect(out.map((p) => p.columnIndex).sort()).toEqual([0, 1]);
    expect(out[0].heightFraction).toBe(0);
  });

  it('clamps out-of-range minutes', () => {
    const [p] = layoutDayColumn([span(-30, MINUTES_PER_DAY + 120)]);
    expect(p.topFraction).toBe(0);
    expect(p.heightFraction).toBe(1);
  });

  it('handles an empty day', () => {
    expect(layoutDayColumn([])).toEqual([]);
  });
});

describe('layoutDayColumn — visible window', () => {
  const W7to23 = { startMin: at(7), endMin: at(23) }; // 420..1380, span 960 min
  const winMin = at(23) - at(7);

  it('defaults to the full day (placement "in", full-day fractions)', () => {
    const [p] = layoutDayColumn([span(at(9), at(10))]);
    expect(p.placement).toBe('in');
    expect(p.topFraction).toBeCloseTo(at(9) / MINUTES_PER_DAY);
  });

  it('positions an in-window event relative to the window', () => {
    const [p] = layoutDayColumn([span(at(9), at(10, 30))], W7to23);
    expect(p.placement).toBe('in');
    expect(p.topFraction).toBeCloseTo((at(9) - at(7)) / winMin);
    expect(p.heightFraction).toBeCloseTo(90 / winMin);
  });

  it('clamps a partly-before event to the window top (keeps its in-window slice)', () => {
    // 6:00–9:00, window 7–23 → shows 7:00–9:00.
    const [p] = layoutDayColumn([span(at(6), at(9))], W7to23);
    expect(p.placement).toBe('in');
    expect(p.topFraction).toBeCloseTo(0);
    expect(p.heightFraction).toBeCloseTo((at(9) - at(7)) / winMin);
  });

  it('clamps a partly-after event to the window bottom', () => {
    // 22:00–24:00, window 7–23 → shows 22:00–23:00 at the bottom.
    const [p] = layoutDayColumn([span(at(22), MINUTES_PER_DAY)], W7to23);
    expect(p.placement).toBe('in');
    expect(p.topFraction).toBeCloseTo((at(22) - at(7)) / winMin);
    expect(p.heightFraction).toBeCloseTo((at(23) - at(22)) / winMin);
  });

  it('flags entirely-outside events "before" / "after"', () => {
    const out = layoutDayColumn(
      [span(at(2), at(3)), span(at(23, 30), at(23, 45))],
      W7to23,
    );
    expect(out[0].placement).toBe('before');
    expect(out[1].placement).toBe('after');
  });

  it('an event ending exactly at the window start is "before"; a point AT the start is "in"', () => {
    const out = layoutDayColumn([span(at(6), at(7)), span(at(7), at(7))], W7to23);
    expect(out[0].placement).toBe('before');
    expect(out[1].placement).toBe('in');
    expect(out[1].topFraction).toBeCloseTo(0);
  });

  it('a point at the (exclusive) window end is "after"', () => {
    const [p] = layoutDayColumn([span(at(23), at(23))], W7to23);
    expect(p.placement).toBe('after');
  });

  it('computes overlap within the clamped window', () => {
    const out = layoutDayColumn(
      [span(at(9), at(10, 30)), span(at(10), at(11))],
      W7to23,
    );
    expect(out.map((p) => p.columnCount)).toEqual([2, 2]);
    expect(out.every((p) => p.placement === 'in')).toBe(true);
  });

  it('keeps input order with mixed in/before/after items', () => {
    const out = layoutDayColumn(
      [span(at(9), at(10)), span(at(2), at(3)), span(at(23, 30), at(23, 45))],
      W7to23,
    );
    expect(out.map((p) => p.placement)).toEqual(['in', 'before', 'after']);
  });
});

describe('dropMinuteInWindow', () => {
  const FULL = { startMin: 0, endMin: MINUTES_PER_DAY };
  const WINDOW = { startMin: at(7), endMin: at(23) }; // 7:00–23:00

  it('maps the fraction linearly into the window, snapped to 15 min', () => {
    // Halfway down a 7–23 window is 15:00 exactly.
    expect(dropMinuteInWindow(0.5, WINDOW)).toBe(at(15));
    // A bit past halfway snaps to the NEAREST quarter hour, either way.
    expect(dropMinuteInWindow(0.505, WINDOW)).toBe(at(15));
    expect(dropMinuteInWindow(0.52, WINDOW)).toBe(at(15, 15));
  });

  it('clamps to the window edges', () => {
    expect(dropMinuteInWindow(-0.2, WINDOW)).toBe(at(7));
    expect(dropMinuteInWindow(0, WINDOW)).toBe(at(7));
    // The bottom edge is the window end — a valid wall-clock time here.
    expect(dropMinuteInWindow(1, WINDOW)).toBe(at(23));
    expect(dropMinuteInWindow(1.5, WINDOW)).toBe(at(23));
  });

  it('never yields 24:00 on the full-day window', () => {
    // The exclusive day end is not a schedulable minute; the bottom of a
    // full-day grid lands on the last snap step instead.
    expect(dropMinuteInWindow(1, FULL)).toBe(MINUTES_PER_DAY - 15);
  });

  it('tolerates a non-finite fraction (degenerate geometry)', () => {
    expect(dropMinuteInWindow(Number.NaN, WINDOW)).toBe(at(7));
  });
});

describe('minutesFromMidnight', () => {
  it('parses HH:MM and HH:MM:SS', () => {
    expect(minutesFromMidnight('09:30')).toBe(570);
    expect(minutesFromMidnight('9:05')).toBe(545);
    expect(minutesFromMidnight('23:59:45')).toBe(23 * 60 + 59);
    expect(minutesFromMidnight('00:00')).toBe(0);
  });
  it('rejects garbage', () => {
    expect(minutesFromMidnight('24:00')).toBeNull();
    expect(minutesFromMidnight('9')).toBeNull();
    expect(minutesFromMidnight('foo')).toBeNull();
  });
});

describe('eventBlockFactor', () => {
  it('returns the floor factor (1) for zero/negative/NaN and sub-hour durations', () => {
    expect(eventBlockFactor(0)).toBe(1);
    expect(eventBlockFactor(-30)).toBe(1);
    expect(eventBlockFactor(Number.NaN)).toBe(1);
    expect(eventBlockFactor(Number.POSITIVE_INFINITY)).toBe(1);
    expect(eventBlockFactor(30)).toBe(1); // sub-hour floored to one line
  });

  it('is ~linear in hours above the floor (a 4h event is 4x a 1h event)', () => {
    expect(eventBlockFactor(60)).toBeCloseTo(1);
    expect(eventBlockFactor(90)).toBeCloseTo(1.5);
    expect(eventBlockFactor(120)).toBeCloseTo(2);
    expect(eventBlockFactor(240)).toBeCloseTo(4);
  });

  it('is monotonic non-decreasing', () => {
    expect(eventBlockFactor(30)).toBeLessThanOrEqual(eventBlockFactor(60));
    expect(eventBlockFactor(60)).toBeLessThan(eventBlockFactor(120));
    expect(eventBlockFactor(120)).toBeLessThan(eventBlockFactor(180));
  });

  it('caps a very long event so the block stays bounded', () => {
    expect(eventBlockFactor(360)).toBeCloseTo(6);
    expect(eventBlockFactor(600)).toBe(6);
    expect(eventBlockFactor(MINUTES_PER_DAY)).toBe(6);
  });
});
