import { describe, expect, it } from 'vitest';

import {
  compareNames,
  compareTitles,
  filterTasksOnDay,
  sortDayMarkers,
  type DayMarker,
  type Task,
} from '@aperio/shared';

/**
 * The text ordering, exercised through the door the desktop actually uses.
 *
 * `src/test-setup.ts` installs the real WebAssembly module as the surface's
 * `TextCollation`, so every assertion here runs `cal_core::collation` — the
 * same code the app runs, not a stand-in. The rules themselves are pinned in
 * Rust (`crates/cal-core/src/collation.rs`); what this file pins is the CHAIN:
 * that `@aperio/shared`'s comparators reach it, and that the two lists a user
 * reads most come out in the order the core decided.
 *
 * There is deliberately no parity test against `localeCompare` here, unlike
 * `coreRules.parity.test.ts`. `localeCompare` is what this replaced, and its
 * answer depends on the runtime — pinning Rust to it would pin the app to
 * whichever collation the CI's Node happens to ship.
 */
describe('text ordering comes from the core', () => {
  it('files umlauts with their base letter, not behind every ASCII word', () => {
    // The case German lists are full of and the one codepoint order gets
    // worst: `Ä` is U+00C4, so a naive compare files "Äpfel" after "Zebra".
    expect(compareTitles('Äpfel', 'Zebra')).toBeLessThan(0);
    expect(compareNames('Ärger', 'Zebra')).toBeLessThan(0);
  });

  it('orders digit runs in titles by value', () => {
    expect(compareTitles('Kapitel 2', 'Kapitel 10')).toBeLessThan(0);
    expect(compareTitles('Kapitel 10', 'Kapitel 2')).toBeGreaterThan(0);
  });

  it('treats two spellings of one NAME as one name', () => {
    // What `sensitivity: 'base'` meant at the call sites this replaced: a
    // sidebar mixing "Arbeit" and "arbeit" must not split into two blocks.
    expect(compareNames('Arbeit', 'arbeit')).toBe(0);
  });

  it('keeps two TITLES apart when only their case differs', () => {
    // Unlike names: answering equal would make their order depend on which
    // was read first, which is how a list reshuffles itself between renders.
    expect(compareTitles('Arbeit', 'arbeit')).not.toBe(0);
  });

  it('orders a task list by priority band, then naturally by title', () => {
    // `priority` is REQUIRED on `Task`, and this fixture omitted it — the
    // `as Task` cast hid that. It went unnoticed while the ranking was
    // TypeScript, because a `switch` with no matching case returned
    // `undefined`, `undefined - undefined` is NaN, and `NaN || …` fell through
    // to the title compare. The Rust rule throws instead, which is how the
    // gap surfaced: a comparator that silently reorders is the failure mode
    // this whole exercise keeps finding.
    const DAY = '2026-05-20';
    const task = (id: string, title: string): Task =>
      ({ id, list_id: 'L1', title, status: 'open', priority: 'medium', scheduled_date: DAY }) as Task;
    // Through the calendar-day door: the ordering is the core's now.
    const sorted = filterTasksOnDay(
      [task('c', 'Übung 10'), task('a', 'Übung 2'), task('b', 'Aufgabe')],
      DAY,
    ).map((t) => t.title);
    expect(sorted).toEqual(['Aufgabe', 'Übung 2', 'Übung 10']);
  });

  it('orders day markers by position first, then by name', () => {
    const marker = (id: string, name: string, position?: number): DayMarker => ({
      id,
      name,
      position,
    });
    expect(
      sortDayMarkers([
        marker('c', 'Ärztin', 1),
        marker('a', 'zuhause', 1),
        marker('b', 'Sport', 0),
      ]).map((m) => m.name),
    ).toEqual(['Sport', 'Ärztin', 'zuhause']);
  });
});
