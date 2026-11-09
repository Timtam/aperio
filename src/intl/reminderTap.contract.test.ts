import { describe, expect, it } from 'vitest';

import contract from '../../shared/contracts/reminderTap.json';
import { reminderTapData, reminderTapOf, type TapSource } from '../../shared/reminderTap';

/**
 * The TypeScript half of the reminder-click contract (DESIGN §14.3).
 *
 * Its Rust twin is `tests::tap_contract` in `crates/host-core/src/reminders.rs`
 * and reads the SAME file. The phone writes a notification's data here, with
 * `reminderTapData`; the desktop's reminder worker writes a toast's in Rust,
 * with `Trigger::tap_data`; and `reminderTapOf` reads both. A key spelt
 * differently on one side would make a click open nothing, without a word.
 */
describe('reminder click data, as the fixture pins it', () => {
  for (const c of contract.cases) {
    it(`${c.name}: the phone writes what the desktop writes`, () => {
      expect(reminderTapData(c.source as TapSource)).toEqual(c.data);
    });

    it(`${c.name}: a click reads it back`, () => {
      expect(reminderTapOf(c.data)).toEqual(c.tap);
    });
  }
});
