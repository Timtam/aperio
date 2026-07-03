// Pure geometry for a proportional day/week time-grid, shared by the desktop
// and mobile calendars so the overlap math lives in ONE tested place. Given a
// day's timed items (each a [startMin, endMin] span in minutes from local
// midnight), it returns each item's fractional top + height within the day and
// its side-by-side column placement when items overlap — the classic calendar
// "columns" layout (Google/Outlook/Apple). The renderer turns the fractions
// into pixels and clamps a minimum height so a 5-minute event stays legible.
//
// This is PURELY visual: it never reorders items (the caller keeps its
// chronological DOM order for screen-reader + keyboard navigation), it just
// tells the renderer where to draw each one.

/** Minutes in a day — the denominator for every fraction. */
export const MINUTES_PER_DAY = 24 * 60;

export interface TimedSpan {
  /** Minutes from local midnight of the start, clamped to [0, 1440]. */
  startMin: number;
  /** Minutes from local midnight of the end, clamped to [startMin, 1440]. A
   *  zero-duration point (a timed task, which has a time but no duration) has
   *  `endMin === startMin`; it still gets a column + a min-height chip. */
  endMin: number;
}

/** Where a timed item sits relative to the visible window. `'in'` items are
 *  positioned by the fractions below; `'before'`/`'after'` items fall ENTIRELY
 *  outside the window — the renderer collects them into the top/bottom "outside
 *  hours" band and ignores their fractions (which are 0). Items keep their
 *  source/DOM order either way, so screen-reader + keyboard nav reach every item
 *  regardless of the window. */
export type SpanPlacement = 'in' | 'before' | 'after';

export interface PositionedSpan {
  /** Position relative to the visible window. */
  placement: SpanPlacement;
  /** Top edge as a fraction [0, 1] of the VISIBLE-WINDOW height (0 for an
   *  outside item). */
  topFraction: number;
  /** Height as a fraction (0, 1] of the visible-window height; clamped to the
   *  window so a partly-outside event shows only its in-window slice. May be ~0
   *  for a zero-duration point — the renderer applies a px minimum. */
  heightFraction: number;
  /** This item's column within its overlap cluster (0-based). */
  columnIndex: number;
  /** Total columns in this item's overlap cluster (≥ 1); width = 1/columnCount. */
  columnCount: number;
}

/** The visible time window of the grid, in minutes from local midnight. The
 *  default `FULL_DAY_WINDOW` spans the whole day (the historical behaviour). */
export interface DayWindow {
  /** Window start, minutes from midnight (0 = midnight). */
  startMin: number;
  /** Window end, minutes from midnight (1440 = end of day). Exclusive bottom edge. */
  endMin: number;
}

export const FULL_DAY_WINDOW: DayWindow = { startMin: 0, endMin: MINUTES_PER_DAY };

/** Clamp a raw minute value into a valid in-day position. */
function clampMinute(min: number): number {
  if (!Number.isFinite(min)) return 0;
  if (min < 0) return 0;
  if (min > MINUTES_PER_DAY) return MINUTES_PER_DAY;
  return min;
}

/**
 * Minute-of-day for a DROP at `fraction` (0 = the window's top edge, 1 = its
 * bottom) of the visible grid window, snapped to `stepMin` (default 15 — fine
 * enough to feel intentional, coarse enough to hit without pixel-precision).
 * Clamped so the result is a valid wall-clock minute inside the window and
 * never 24:00 (the exclusive day end). Drives the desktop "drag a task onto
 * the hour grid" time assignment.
 */
export function dropMinuteInWindow(
  fraction: number,
  window: DayWindow,
  stepMin = 15,
): number {
  const windowMin = Math.max(1, window.endMin - window.startMin);
  const frac = Number.isFinite(fraction) ? Math.min(1, Math.max(0, fraction)) : 0;
  const raw = window.startMin + frac * windowMin;
  const snapped = Math.round(raw / stepMin) * stepMin;
  const top = Math.min(window.endMin, MINUTES_PER_DAY - stepMin);
  return Math.max(window.startMin, Math.min(top, snapped));
}

/**
 * The local minutes-from-midnight span an event occupies on `day`, clamped to
 * the day so a multi-day event clips to [0, 1440]. `day` is any instant on the
 * target day; the span is measured from that day's LOCAL midnight. Shared by the
 * desktop + mobile calendars so the grid/list geometry stays in one place (the
 * callers pass `new Date(ev.start)` / `new Date(ev.end)`).
 */
export function eventSpanForDay(start: Date, end: Date, day: Date): TimedSpan {
  const base = new Date(day);
  base.setHours(0, 0, 0, 0);
  const baseMs = base.getTime();
  return {
    startMin: clampMinute(Math.round((start.getTime() - baseMs) / 60000)),
    endMin: clampMinute(Math.round((end.getTime() - baseMs) / 60000)),
  };
}

/**
 * Lay out a single day-column's timed items. Input order is preserved in the
 * OUTPUT (result[i] positions input[i]); the algorithm sorts internally only to
 * compute overlaps. Overlap rule: two spans overlap when one starts strictly
 * before the other ends (touching edges — one ends exactly when the next starts
 * — do NOT overlap, so back-to-back meetings sit full-width). A zero-duration
 * point is treated as an infinitesimally short span for overlap, so two tasks
 * at the same minute share two columns but a task at the boundary of an event
 * stays clear.
 */
export function layoutDayColumn(
  spans: TimedSpan[],
  window: DayWindow = FULL_DAY_WINDOW,
): PositionedSpan[] {
  const n = spans.length;
  const result: PositionedSpan[] = new Array(n);

  const winStart = clampMinute(window.startMin);
  // Keep a strictly-positive window so the fraction denominator is never 0.
  const winEnd = Math.max(winStart + 1, clampMinute(window.endMin));
  const winMin = winEnd - winStart;

  const outside = (placement: SpanPlacement): PositionedSpan => ({
    placement,
    topFraction: 0,
    heightFraction: 0,
    columnIndex: 0,
    columnCount: 1,
  });

  // A zero-duration point gets an infinitesimal EFFECTIVE end (< 1 minute, the
  // smallest real gap) used ONLY for overlap math, so two coincident points
  // split into side-by-side columns while a point at an event's edge — or two
  // back-to-back events — still read as non-overlapping. The real `endMin`
  // (which may equal startMin) still drives the rendered height.
  const EPSILON = 1e-3;

  // Classify each item against the window. Items entirely before/after it are
  // banded by the renderer (placement flag, fractions ignored) and don't
  // compete for columns; in-window items are CLAMPED to the window for both the
  // overlap pass and their rendered fractions. Each in-window item carries its
  // ORIGINAL index so the output stays aligned with the input order.
  const items: { i: number; startMin: number; endMin: number; effEnd: number }[] = [];
  spans.forEach((s, i) => {
    const startMin = clampMinute(s.startMin);
    const endMin = Math.max(startMin, clampMinute(s.endMin));
    const isPoint = endMin === startMin;
    // Before: ends at/before the window start (a point strictly before it). A
    // point exactly at winStart stays IN (at the top edge).
    if (isPoint ? startMin < winStart : endMin <= winStart) {
      result[i] = outside('before');
      return;
    }
    // After: starts at/after the (exclusive) window end.
    if (startMin >= winEnd) {
      result[i] = outside('after');
      return;
    }
    const visStart = Math.max(startMin, winStart);
    const visEnd = Math.min(endMin, winEnd);
    const effEnd = visEnd === visStart ? visStart + EPSILON : visEnd;
    items.push({ i, startMin: visStart, endMin: visEnd, effEnd });
  });

  // Sort by start, then by effective end (longer first on a tie) for a stable
  // greedy column pass.
  const order = items
    .slice()
    .sort((a, b) => a.startMin - b.startMin || b.effEnd - a.effEnd);

  // Walk the sorted items, accumulating a "cluster" of transitively-overlapping
  // items. A cluster closes when the next item starts at/after the max effective
  // end seen so far — then every item in it shares the same columnCount.
  let cluster: { item: (typeof items)[number]; col: number }[] = [];
  let clusterMaxEnd = -1;

  const flush = () => {
    if (cluster.length === 0) return;
    const cols = cluster.reduce((m, c) => Math.max(m, c.col + 1), 0);
    for (const c of cluster) {
      result[c.item.i] = {
        placement: 'in',
        topFraction: (c.item.startMin - winStart) / winMin,
        heightFraction: (c.item.endMin - c.item.startMin) / winMin,
        columnIndex: c.col,
        columnCount: cols,
      };
    }
    cluster = [];
    clusterMaxEnd = -1;
  };

  // `colEnds[c]` = effective end of the last span placed in column c (current
  // cluster only). A span goes in the first column whose last span has ended.
  let colEnds: number[] = [];

  for (const cur of order) {
    const startsNewCluster = cluster.length > 0 && cur.startMin >= clusterMaxEnd;
    if (startsNewCluster) {
      flush();
      colEnds = [];
    }

    // First column whose last span ended at/before this start (touching is OK).
    let col = colEnds.findIndex((end) => end <= cur.startMin);
    if (col === -1) {
      col = colEnds.length;
    }
    colEnds[col] = cur.effEnd;
    cluster.push({ item: cur, col });
    clusterMaxEnd = Math.max(clusterMaxEnd, cur.effEnd);
  }
  flush();

  return result;
}

/**
 * Convert a local wall-clock `HH:MM[:SS]` (or a Date) to minutes from midnight.
 * Returns null for an unparseable time. Seconds are floored into the minute.
 */
export function minutesFromMidnight(time: string): number | null {
  const m = /^(\d{1,2}):(\d{2})(?::(\d{2}))?$/.exec(time.trim());
  if (!m) return null;
  const hh = Number(m[1]);
  const mm = Number(m[2]);
  if (hh > 23 || mm > 59) return null;
  return hh * 60 + mm;
}

// ── Compact list view ───────────────────────────────────────────────────────
// The lighter alternative to the hour-grid: a chronological LIST where each
// event block's size still reflects its DURATION (a long meeting reads as a
// bigger block than a short one) — but BOUNDED, so an all-afternoon event
// doesn't dwarf the rest. This returns a unitless size FACTOR; each platform
// multiplies it by its own base unit (em on desktop CSS, px in React Native)
// so the SHAPE of the curve lives in one tested place while the absolute scale
// stays platform-native. Tasks in the list view are sized by EFFORT instead
// (they have no duration), via the existing effort-size classes/styles.

/** Floor factor: events at/under 1 hour share the one-line minimum so a short
 *  event stays legible (and the block height never drops below one line). */
const LIST_BLOCK_MIN_FACTOR = 1;
/** Hard cap so a very long event stays bounded (≈ a 6h+ block). */
const LIST_BLOCK_MAX_FACTOR = 6;

/**
 * Size factor for a list-view event block — roughly the duration in HOURS, so
 * the rendered height reads the duration RATIO at a glance (a 4h event is ≈ 4×
 * a 1h event). Floored at LIST_BLOCK_MIN_FACTOR (events ≤ 1h share the one-line
 * minimum for legibility) and capped at LIST_BLOCK_MAX_FACTOR. A zero/negative/
 * NaN duration (or a point) returns the floor. Multiply by a per-surface
 * one-line base unit to get the (STRICT) block height.
 */
export function eventBlockFactor(durationMin: number): number {
  if (!Number.isFinite(durationMin) || durationMin <= 0) {
    return LIST_BLOCK_MIN_FACTOR;
  }
  const hours = durationMin / 60;
  return Math.min(Math.max(hours, LIST_BLOCK_MIN_FACTOR), LIST_BLOCK_MAX_FACTOR);
}
