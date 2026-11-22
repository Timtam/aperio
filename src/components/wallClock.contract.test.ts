import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/wallClock.json';
import { askWallClock, instantOf, untilOn, wallOf, type WallClockQuestion } from '@aperio/shared';
import { moduleTzdataVersion } from '../wasm/coreRules';

/**
 * Dates and times on the clock a series repeats on, pinned by
 * `crates/cal-core/tests/fixtures/wallClock.json` (written from tzdata's rules
 * and checked against Node's ICU data). This replays every row through the
 * door this surface installs, the real WebAssembly module, which carries the
 * zones since stage 8 (decision 100a); the core and the phone's bridge read
 * the same file.
 */
describe('wallClock contract', () => {
  // Anti-silence: the rows the rule turns on, by name.
  it('still carries the rows the rule turns on', () => {
    const names = new Set(contract.rows.map((row) => row.name));
    for (const needed of [
      'berlin-spring-gap',
      'berlin-autumn-fold',
      'lord-howe-half-hour-gap',
      'apia-skipped-day',
      'paris-1900-seconds',
      'until-new-york',
      'no-zone-is-utc',
      'alias-calcutta',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const row of contract.rows) {
    it(`answers ${row.name}`, () => {
      expect(askWallClock(row.question as WallClockQuestion)).toEqual(row.expect);
    });
  }

  it('asks through the named helpers alike', () => {
    expect(wallOf('2026-11-02T08:00:00Z', 'Europe/Berlin')).toMatchObject({
      date: '2026-11-02',
      time: '09:00',
    });
    expect(instantOf('2027-03-28', '02:30', 'Europe/Berlin')?.wall).toEqual({
      reading: 'gap',
      offsetBefore: 3600,
      offsetAfter: 7200,
    });
    expect(untilOn('2026-11-30', 'America/New_York')?.at).toBe('2026-12-01T04:59:59Z');
  });
});

describe('the zones in the WebAssembly module (decision 104a)', () => {
  // The desktop checks at startup that the module and its host name the same
  // tzdata release. This holds the module to the release the repository pins,
  // so a module built from other data fails here first.
  it('come from the tzdata release the repository pins', () => {
    const pinned = readFileSync('crates/cal-core/tzdata/version', 'utf8').trim();
    expect(moduleTzdataVersion()).toBe(pinned);
  });
});
