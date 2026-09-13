import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/signatures.json';
import {
  answerApply,
  answerRead,
  answerStrip,
  type ApplyInput,
  type TextInput,
} from './signatures.contractSupport';

/**
 * The signature block, pinned as a table before it moves.
 *
 * `signatureIn` reads the block, `stripSignature` removes it, `applySignature`
 * writes one (replacing, never stacking). Today that is TypeScript, run by
 * both editors on both surfaces; it is going to be asked of `cal-core`
 * instead, and the Rust answer has to be the same bytes. This file replays
 * every case through the TypeScript — the Rust side reads the same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
describe('signatures contract', () => {
  // Anti-silence: named rows, not a count. Each is the one case a whole rule
  // turns on; a fixture that lost it would pass for the wrong reason.
  it('still carries the rows the rules turn on', () => {
    const texts = new Set(contract.texts.map((c) => c.name));
    for (const needed of [
      'the-last-marker-wins',
      'a-marker-whose-trailing-space-a-provider-ate',
      'an-indented-marker-is-no-marker',
      'several-blank-lines-before-the-block-all-go',
      'trailing-nel-on-the-marker-line',
      'trailing-bom-on-the-marker-line',
    ]) {
      expect(texts, `fixture lost ${needed}`).toContain(needed);
    }
    const apply = new Set(contract.apply.map((c) => c.name));
    for (const needed of [
      'applying-twice-leaves-one-block',
      'replaces-the-block-when-the-signature-changes',
      'a-whitespace-description-is-nothing',
      'an-empty-body-removes-the-block',
      'a-bom-only-description-is-nothing',
      'a-nel-only-description-is-text',
    ]) {
      expect(apply, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const c of contract.texts) {
    it(`read: ${c.name}`, () => {
      expect(answerRead(c.input as TextInput), c.note).toEqual(c.expect.read);
    });
    it(`strip: ${c.name}`, () => {
      expect(answerStrip(c.input as TextInput), c.note).toEqual(c.expect.strip);
    });
  }

  for (const c of contract.apply) {
    it(`apply: ${c.name}`, () => {
      expect(answerApply(c.input as ApplyInput), c.note).toEqual(c.expect);
    });
  }
});
