import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/calendarWeek.json';
import { calendarWeekOf, calendarWeeks } from '@aperio/shared';

/**
 * The calendar week a day is shown in, through this surface's door into the
 * core (`cal_core::calendar_week`): the fixture the core's own contract test
 * reads, computed with Python's `date.isocalendar()`, independently of both.
 * The phone asks the same rule through UniFFI.
 */
describe('the calendar week through the door into the core', () => {
  it.each(contract.cases)('$name', ({ day, weekStartsOn, expect: expected }) => {
    expect(calendarWeekOf(day, weekStartsOn)).toEqual(expected);
  });

  it('answers many days in one question, marking where each week begins', () => {
    const { days, weekStartsOn, expect: expected } = contract.batch;
    expect(calendarWeeks(days, weekStartsOn)).toEqual(expected);
  });

  it('wraps a setting outside 0 to 6 the way the backlog weeks do', () => {
    expect(calendarWeekOf('2026-11-01', -1)).toEqual(calendarWeekOf('2026-11-01', 6));
  });
});
