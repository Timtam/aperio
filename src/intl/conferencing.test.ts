import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/conferencing.json';
import { detectConference, type ConferenceLink } from '@aperio/shared';

/**
 * Conference detection, exercised through the door the desktop actually uses.
 *
 * This replaces 231 lines that tested a TypeScript implementation of the rule.
 * That implementation is gone: it was a second copy of `cal_core::conferencing`,
 * which has been in production all along, and reading the two side by side
 * turned up six places where they had drifted apart.
 *
 * What runs here is the real thing. `src/test-setup.ts` installs the real
 * WebAssembly module as the surface's `ConferenceDetector`, so every case below
 * crosses the same JSON boundary a running app crosses.
 *
 * The cases come from `crates/cal-core/tests/fixtures/conferencing.json` — the
 * SAME file the Rust contract test reads. That is the point: a rule with one
 * implementation still has two ways to reach it, and this is the one that proves
 * the door carries every case intact. The Rust test proves the rule; this proves
 * the crossing.
 *
 * The fixture also records, per row, what the deleted TypeScript answered and
 * why the surviving answer was chosen — so the behaviour this migration changed
 * is readable rather than archaeological.
 */
interface ContractCase {
  name: string;
  location: string | null;
  description: string | null;
  expect: Record<string, unknown> | null;
}

describe('conference detection comes from the core', () => {
  const cases = contract.cases as unknown as ContractCase[];

  it('reads the same fixture the Rust contract test reads', () => {
    // Anti-silence: an import that resolved to an empty object would make every
    // case below vacuous, and the suite would stay green while proving nothing.
    // Named rather than counted — adding a row is the change this must survive.
    const names = cases.map((c) => c.name);
    for (const must of [
      'webex-dtmf-canonical',
      'turkish-dotted-capital-i-before-a-uri',
      'two-links-of-equal-length-one-with-an-umlaut',
      'no-meeting-at-all',
    ]) {
      expect(names).toContain(must);
    }
  });

  it.each(cases.map((c) => [c.name, c] as const))(
    'answers the contract for %s',
    (_name, testCase) => {
      const found = detectConference({
        location: testCase.location,
        description: testCase.description,
      });

      if (testCase.expect === null) {
        expect(found).toBeNull();
        return;
      }
      expect(found).not.toBeNull();
      const link = found as ConferenceLink;

      expect(link.joinUrl).toBe(testCase.expect.join_url);
      expect(link.provider).toBe(testCase.expect.provider);
      expect(link.source).toBe(testCase.expect.source);

      // Absent in the fixture means absent in the answer. Spelling it that way
      // round is what makes a field that appears out of nowhere fail here
      // rather than pass unnoticed.
      for (const [fixtureKey, actual] of [
        ['meeting_number', link.meetingNumber],
        ['password', link.password],
        ['sip_address', link.sipAddress],
        ['phone', link.phone],
      ] as const) {
        expect(actual ?? null).toBe(testCase.expect[fixtureKey] ?? null);
      }

      expect(link.labelledDetails).toEqual(
        testCase.expect.labelled_details ?? [],
      );
    },
  );
});
