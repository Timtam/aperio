import { describe, expect, it } from 'vitest';

import { pickerMisreadsRule, rebuiltByPicker } from '@aperio/shared';

/**
 * Decision 87b: where the repeat controls would show a different rule than
 * the one stored, the editor says the stored rule in words above them.
 *
 * The controls parse a rule into their own options and write those options
 * back. For a rule they cannot hold, what they show is a different rule — and
 * touching any control saves it. That is a silent lie, and this is how the
 * editor knows to say something instead.
 */

describe('pickerMisreadsRule', () => {
  it('is false for the rules the controls can hold', () => {
    for (const rule of [
      'FREQ=DAILY',
      'FREQ=WEEKLY;BYDAY=MO,WE',
      'FREQ=WEEKLY;INTERVAL=2;BYDAY=TU',
      'FREQ=MONTHLY;BYMONTHDAY=15',
      'FREQ=MONTHLY;BYDAY=2TU',
      'FREQ=YEARLY',
      'FREQ=YEARLY;BYMONTH=3;BYDAY=2TU',
      'FREQ=WEEKLY;BYDAY=MO;COUNT=5',
      // The last day of the month round-trips; the controls hold it.
      'FREQ=MONTHLY;BYMONTHDAY=-1',
      // The defaults a provider spells out and the pickers leave implicit.
      'FREQ=WEEKLY;INTERVAL=1;BYDAY=TU',
      'FREQ=WEEKLY;WKST=MO;BYDAY=TU',
      'FREQ=DAILY;INTERVAL=1',
      'RRULE:FREQ=DAILY',
    ]) {
      expect(pickerMisreadsRule(rule)).toBe(false);
    }
    // No rule at all is not a misreading.
    expect(pickerMisreadsRule(null)).toBe(false);
    expect(pickerMisreadsRule('')).toBe(false);
  });

  it('is true for the rules they would show as something else', () => {
    for (const rule of [
      // Read as "does not repeat": the controls would drop the rule.
      'freq=weekly;byday=mo',
      'FREQ=HOURLY',
      // "The last workday of the month" becomes "the last Monday".
      'FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1',
      // A part the controls do not carry at all, dropped in silence.
      'FREQ=WEEKLY;BYDAY=MO;BYWEEKNO=3',
    ]) {
      expect(pickerMisreadsRule(rule)).toBe(true);
    }
  });

  it('names what the controls would write instead', () => {
    // "The last workday" would be saved as "the last Monday".
    expect(rebuiltByPicker('FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1')).toBe(
      'FREQ=MONTHLY;BYDAY=-1MO',
    );
    // And a rule they cannot read at all as no repeat.
    expect(rebuiltByPicker('freq=weekly;byday=mo')).toBeNull();
  });
});
