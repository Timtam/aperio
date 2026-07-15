// @vitest-environment jsdom
//
// Regression guard for the "Google shared calendar events multiply until you
// switch views/restart" bug. The calendar views stay MOUNTED while their event
// set changes (day-to-day navigation, background refresh, calendar toggle). When
// the same group event arrives under several calendars with a byte-identical id
// (Google reuses one event id across attendee/shared copies) and rows are keyed
// by that id alone, React's duplicate-key reconciliation orphans DOM nodes that
// PILE UP across re-renders and only clear on unmount. `eventInstanceKey` keys by
// (calendar_id, id) so every real row is unique and the count stays exact.
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { eventInstanceKey } from '@aperio/shared';

interface Ev {
  id: string;
  calendar_id: string;
}

function Listbox({ items, keyer }: { items: Ev[]; keyer: (e: Ev) => string }) {
  return (
    <ul role="listbox">
      {items.map((ev) => (
        <li role="option" key={keyer(ev)} aria-label={`${ev.calendar_id}:${ev.id}`}>
          {ev.calendar_id}:{ev.id}
        </li>
      ))}
    </ul>
  );
}

// One group event shared across two subscribed calendars (identical id) + a
// unique event; every third "day" has only the unique event.
const day = (n: number): Ev[] => [
  { id: `shared@day${n}`, calendar_id: 'A' },
  { id: `shared@day${n}`, calendar_id: 'B' },
  { id: `u${n}`, calendar_id: 'A' },
];
const sparseDay = (n: number): Ev[] => [{ id: `u${n}`, calendar_id: 'A' }];

// Navigate day-to-day WITHOUT unmounting (the real trigger) and return the
// option count after each day.
function navigate(keyer: (e: Ev) => string): number[] {
  const { rerender } = render(<Listbox items={day(0)} keyer={keyer} />);
  const counts: number[] = [];
  for (let n = 1; n <= 12; n += 1) {
    rerender(<Listbox items={n % 3 === 0 ? sparseDay(n) : day(n)} keyer={keyer} />);
    counts.push(screen.getAllByRole('option').length);
  }
  return counts;
}

describe('calendar-view row keys survive same-id events across calendars', () => {
  const expected = Array.from({ length: 12 }, (_, i) => ((i + 1) % 3 === 0 ? 1 : 3));

  it('DEMONSTRATES the bug: keying by id alone inflates the row count', () => {
    const counts = navigate((e) => e.id);
    // The buggy path orphans nodes: the final count exceeds the correct one.
    expect(counts[counts.length - 1]).toBeGreaterThan(expected[expected.length - 1]);
  });

  it('eventInstanceKey keeps the count exact across navigation', () => {
    const counts = navigate((e) => eventInstanceKey(e));
    expect(counts).toEqual(expected);
  });
});
