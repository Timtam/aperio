import { describe, expect, it } from 'vitest';

import {
  compactDaySummary,
  dayLogIsEmpty,
  moveDayMarker,
  resolveDayMarkers,
  sortDayMarkers,
  spokenDaySummary,
  toggleDayMarker,
  type DayLog,
  type DayMarker,
} from '@aperio/shared';

/** A stored day. `updated_at` is required on the type because the backend
 *  rejects a log without one — the tests build real payloads, not shapes. */
const logOf = (day: string, markers: string[]): DayLog => ({
  day,
  markers,
  updated_at: '2026-08-17T06:00:00Z',
});

const vocab: DayMarker[] = [
  { id: 'sport', name: 'Sport', symbol: '🏃', position: 0 },
  { id: 'read', name: 'Gelesen', symbol: '📖', position: 1 },
  { id: 'quiet', name: 'Ruhiger Tag', position: 2 },
];

describe('day markers', () => {
  it('reads a day in the vocabulary order, not the ticking order', () => {
    // The user arranged their markers; a day should read back that way, not
    // in whatever order they happened to tick them that morning.
    const log = logOf('2026-08-17', ['quiet', 'sport']);
    expect(resolveDayMarkers(log, vocab).map((m) => m.id)).toEqual([
      'sport',
      'quiet',
    ]);
  });

  it('drops an id the vocabulary no longer has', () => {
    // How a deleted marker disappears from history without anything rewriting
    // thousands of stored days.
    const log = logOf('2026-08-17', ['sport', 'deleted-one']);
    expect(resolveDayMarkers(log, vocab).map((m) => m.id)).toEqual(['sport']);
  });

  it('speaks names, never symbols', () => {
    // A screen reader announces an emoji by whatever name it has for it
    // ("man running"), which is not what the user called it.
    const log = logOf('2026-08-17', ['sport', 'quiet']);
    expect(spokenDaySummary(log, vocab, 'Festgehalten')).toBe(
      'Festgehalten: Sport, Ruhiger Tag',
    );
  });

  it('says nothing about a day that says nothing', () => {
    expect(spokenDaySummary(logOf('2026-08-17', []), vocab, 'X')).toBeNull();
    expect(spokenDaySummary(null, vocab, 'X')).toBeNull();
    expect(dayLogIsEmpty(null)).toBe(true);
  });

  it('leaves a symbol-less marker out of the compact form only', () => {
    // A row of initials is noise; the spoken form carries the full truth.
    const log = logOf('2026-08-17', ['sport', 'quiet']);
    expect(compactDaySummary(log, vocab)).toBe('🏃');
    expect(spokenDaySummary(log, vocab, 'X')).toContain('Ruhiger Tag');
  });

  it('toggles both ways', () => {
    const log = logOf('2026-08-17', ['sport']);
    expect(toggleDayMarker(log, 'read').markers).toEqual(['sport', 'read']);
    expect(toggleDayMarker(log, 'sport').markers).toEqual([]);
  });

  it('breaks a position tie by name so the list does not shuffle', () => {
    const tied: DayMarker[] = [
      { id: 'b', name: 'Beta', position: 0 },
      { id: 'a', name: 'Alpha', position: 0 },
    ];
    expect(sortDayMarkers(tied).map((m) => m.id)).toEqual(['a', 'b']);
  });

  it('renumbers every position on a move, leaving no gaps', () => {
    // Positions are a total order. Patching only the two that swapped leaves
    // gaps a later insert lands in the middle of.
    const moved = moveDayMarker(vocab, 'quiet', -1);
    expect(moved.map((m) => m.id)).toEqual(['sport', 'quiet', 'read']);
    expect(moved.map((m) => m.position)).toEqual([0, 1, 2]);
  });

  it('leaves an out-of-range move alone', () => {
    // So a caller can offer "move up" on the first row without special-casing.
    expect(moveDayMarker(vocab, 'sport', -1).map((m) => m.id)).toEqual([
      'sport',
      'read',
      'quiet',
    ]);
  });
});
