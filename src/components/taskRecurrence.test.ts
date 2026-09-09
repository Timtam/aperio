import { describe, expect, it } from 'vitest';

import {
  fromBackend,
  TASK_RECURRENCE_DEFAULT,
  toBackend,
  type TaskRecurrenceValue,
} from './taskRecurrence';

const value = (over: Partial<TaskRecurrenceValue>): TaskRecurrenceValue => ({
  ...TASK_RECURRENCE_DEFAULT,
  ...over,
});

describe('taskRecurrence §9.12 conversion', () => {
  it('defaults anchor/placement/fixed_dates on a plain scheduled rule', () => {
    const out = toBackend(value({ freq: 'WEEKLY', interval: 2 }));
    expect(out).toMatchObject({
      frequency: 'weekly',
      interval: 2,
      anchor: 'from_date',
      placement: 'schedule',
      fixed_dates: null,
    });
  });

  it('allows interval 0 for a backlog rule (immediate resurface)', () => {
    const out = toBackend(
      value({
        freq: 'DAILY',
        interval: 0,
        placement: 'BACKLOG',
        anchor: 'FROM_COMPLETION',
      }),
    );
    expect(out).toMatchObject({
      interval: 0,
      placement: 'backlog',
      anchor: 'from_completion',
    });
  });

  it('clamps a scheduled rule to interval ≥ 1', () => {
    const out = toBackend(value({ freq: 'DAILY', interval: 0 }));
    expect(out?.interval).toBe(1);
  });

  it('emits sanitized fixed_dates and drops out-of-range entries', () => {
    const out = toBackend(
      value({
        freq: 'YEARLY',
        placement: 'BACKLOG',
        fixedDates: [
          { month: 4, day: 1 },
          { month: 13, day: 1 }, // bad month → dropped
          { month: 10, day: 1 },
        ],
      }),
    );
    expect(out?.fixed_dates).toEqual([
      { month: 4, day: 1 },
      { month: 10, day: 1 },
    ]);
  });

  it('returns null fixed_dates when none are set', () => {
    const out = toBackend(value({ freq: 'DAILY' }));
    expect(out?.fixed_dates).toBeNull();
  });

  it('parses the new axes from the backend, defaulting when absent', () => {
    const fromMinimal = fromBackend({ frequency: 'daily', interval: 1 });
    expect(fromMinimal.anchor).toBe('FROM_DATE');
    expect(fromMinimal.placement).toBe('SCHEDULE');
    expect(fromMinimal.fixedDates).toEqual([]);

    const fromFull = fromBackend({
      frequency: 'yearly',
      interval: 0,
      anchor: 'from_completion',
      placement: 'backlog',
      fixed_dates: [{ month: 10, day: 1 }],
    });
    expect(fromFull.anchor).toBe('FROM_COMPLETION');
    expect(fromFull.placement).toBe('BACKLOG');
    expect(fromFull.interval).toBe(0); // backlog keeps 0
    expect(fromFull.fixedDates).toEqual([{ month: 10, day: 1 }]);
  });

  /**
   * The data-loss case. A CalDAV VTODO carrying `RRULE:FREQ=WEEKLY;COUNT=10`
   * reaches the frontends as `{type:'after', occurrences:10}` — `cal-core`'s
   * `RecurrenceEnd` has had that variant all along, and
   * `crates/cal-core/src/recurrence.rs` both reads and writes it.
   *
   * The form model knew only "never" and "on a date". So opening such a task,
   * changing the title and saving turned a ten-occurrence series into an
   * endless one — silently, and at the provider too, because
   * `TaskDialog.tsx` calls `recurrenceToBackend` on every save
   * unconditionally. Several adapters declare `recurrence.count = true`, so
   * the case is reachable rather than theoretical.
   */
  it('keeps a COUNT series end across the form round-trip', () => {
    const restored = fromBackend({
      frequency: 'weekly',
      interval: 1,
      day_of_week: null,
      day_of_month: null,
      end: { type: 'after', occurrences: 10 },
      anchor: 'from_date',
      placement: 'schedule',
      fixed_dates: null,
    });
    expect(restored.endMode).toBe('COUNT');
    expect(restored.count).toBe(10);
    expect(toBackend(restored)?.end).toEqual({
      type: 'after',
      occurrences: 10,
    });
  });

  it('does not invent a count when the series has another end', () => {
    // The counter carries a default so the field has something to show the
    // moment the user picks "after count". That default must not leak into a
    // rule that ends never or on a date.
    expect(toBackend(value({ freq: 'DAILY' }))?.end).toEqual({ type: 'never' });
    expect(
      toBackend(value({ freq: 'DAILY', endMode: 'UNTIL', until: '2026-12-31' }))
        ?.end,
    ).toEqual({ type: 'on_date', date: '2026-12-31' });
  });

  it('clamps a nonsensical count rather than writing it', () => {
    // Zero or negative occurrences would be a series that never runs. The
    // backend has no representation for that and neither should the form.
    expect(
      toBackend(value({ freq: 'DAILY', endMode: 'COUNT', count: 0 }))?.end,
    ).toEqual({ type: 'after', occurrences: 1 });
    expect(fromBackend({ frequency: 'daily', end: { type: 'after', occurrences: 0 } }).count).toBe(1);
  });

  it('round-trips a seasonal backlog rule', () => {
    const original = value({
      freq: 'YEARLY',
      placement: 'BACKLOG',
      anchor: 'FROM_COMPLETION',
      fixedDates: [
        { month: 4, day: 1 },
        { month: 10, day: 1 },
      ],
    });
    const restored = fromBackend(toBackend(original));
    expect(restored.placement).toBe('BACKLOG');
    expect(restored.anchor).toBe('FROM_COMPLETION');
    expect(restored.fixedDates).toEqual(original.fixedDates);
  });
});
