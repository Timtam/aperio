import { describe, expect, it } from 'vitest';

import { compareMachineStrings } from '@aperio/shared';

/**
 * Replacing `localeCompare` with a codepoint comparison is only safe where the
 * strings are MACHINE strings — a fixed-shape ISO day key, an RFC-3339
 * instant. This pins that: for every shape the app actually sorts, the two
 * agree.
 *
 * Where they do NOT agree is the interesting half, and it is what decided
 * where this change stopped. It is not punctuation — measured, and a collator
 * orders `a-b` before `ab` exactly as codepoints do. It is CASE: codepoint
 * order puts every uppercase letter before every lowercase one (`B` < `a`),
 * while a collator interleaves them alphabetically (`a` < `B`) and puts the
 * lowercase form of the same letter first (`a` < `A`).
 *
 * That is why the two `PluginsPanel` call sites keep their `localeCompare`.
 * Today's plugin ids and adapter kinds happen to be all-lowercase, so the two
 * would agree — but that list is human-facing, a third-party plugin may put a
 * capital in its id, and the day one does, codepoint order would file it above
 * everything else. The last test below states the real difference rather than
 * the one I first assumed.
 */
describe('compareMachineStrings agrees with collation on machine strings', () => {
  const sign = (n: number) => (n < 0 ? -1 : n > 0 ? 1 : 0);
  const agree = (a: string, b: string) =>
    expect(
      sign(compareMachineStrings(a, b)),
      `${JSON.stringify(a)} vs ${JSON.stringify(b)}`,
    ).toBe(sign(a.localeCompare(b)));

  it('on ISO day keys, including across months and years', () => {
    const days = [
      '2025-12-31',
      '2026-01-01',
      '2026-01-02',
      '2026-01-10',
      '2026-02-01',
      '2026-10-01',
    ];
    for (const a of days) for (const b of days) agree(a, b);
  });

  it('on RFC-3339 instants, including sub-second precision', () => {
    const instants = [
      '2026-01-01T00:00:00Z',
      '2026-01-01T00:00:00.500Z',
      '2026-01-01T00:00:01Z',
      '2026-01-01T09:00:00Z',
      '2026-01-01T10:00:00Z',
      '2026-06-01T23:59:59Z',
    ];
    for (const a of instants) for (const b of instants) agree(a, b);
  });

  it('on the empty string the app substitutes for a missing date', () => {
    // `deadline_date ?? ''` and `updated_at ?? ''` are real call sites, so the
    // empty string has to keep sorting where it did.
    for (const other of ['2026-01-01', '2026-01-01T00:00:00Z']) {
      agree('', other);
      agree(other, '');
    }
    agree('', '');
  });

  it('is a proper comparator: antisymmetric and reflexive', () => {
    const values = ['', '2026-01-01', '2026-01-02', '2026-01-01T00:00:00Z'];
    for (const a of values) {
      expect(compareMachineStrings(a, a)).toBe(0);
      for (const b of values) {
        // `sign`, not `Math.sign`: the latter answers -0 for 0, and `toBe`
        // tells those apart.
        expect(sign(compareMachineStrings(a, b))).toBe(
          sign(-compareMachineStrings(b, a)),
        );
      }
    }
  });

  it('does NOT agree with collation once letter case is involved', () => {
    // The reason the plugin-id call sites keep `localeCompare`. Codepoint
    // order files every capital above every lowercase letter; a collator
    // interleaves them. Neither is wrong — they answer different questions —
    // and swapping one for the other would silently reorder a list a person
    // reads.
    for (const [a, b] of [
      ['A', 'a'],
      ['a', 'B'],
    ]) {
      expect(sign(compareMachineStrings(a, b)), `${a} vs ${b}`).not.toBe(
        sign(a.localeCompare(b)),
      );
    }
  });

  it('agrees on punctuation, which is NOT why the id sites were left alone', () => {
    // Written down because I assumed the opposite and was wrong: a collator
    // does not look past `-` or `.` here, so these agree. Keeping the check
    // means the next reader inherits the measurement instead of the guess.
    for (const [a, b] of [
      ['a-b', 'ab'],
      ['a.b', 'ab'],
      ['com.aperio.ews', 'com.aperio.ical'],
    ]) {
      agree(a, b);
    }
  });
});
