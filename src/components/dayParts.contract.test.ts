import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/dayParts.json';
import { dayParts } from '@aperio/shared';

/**
 * The parts a repeat rule takes from the day a series starts on, pinned by
 * `crates/cal-core/tests/fixtures/dayParts.json` (computed with Python's
 * datetime). This replays every row through the door this surface installs
 * (the real WebAssembly module); the core and the phone's bridge read the
 * same file.
 */
describe('dayParts contract', () => {
  // Anti-silence: the rows the rule turns on, by name.
  it('still carries the rows the rule turns on', () => {
    const days = new Set(contract.rows.map((row) => row.day));
    for (const needed of ['2026-01-31', '2028-02-29', '2026-11-22', '2026-11-24', '2026-12-01']) {
      expect(days, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const row of contract.rows) {
    it(`gives the parts of ${row.day}`, () => {
      expect(dayParts(row.day)).toEqual(row.expect);
    });
  }

  it('gives nothing for a key that names no day', () => {
    for (const key of contract.refused) {
      expect(dayParts(key), JSON.stringify(key)).toBeNull();
    }
  });
});
