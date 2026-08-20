import { describe, expect, it } from 'vitest';

import {
  applySignature,
  SIGNATURE_MARKER,
  signatureIn,
  stripSignature,
} from '@aperio/shared';

const NL = String.fromCharCode(10);
const lines = (...parts: string[]) => parts.join(NL);
const ROOM = lines(
  'Sprechstunde, DFNconf',
  'https://conf.dfn.de/webapp/#/?conference=97912345',
);

describe('applySignature', () => {
  it('appends the block after the appointment text', () => {
    const out = applySignature('Bitte Unterlagen mitbringen.', ROOM);
    expect(out).toBe(
      lines('Bitte Unterlagen mitbringen.', '', SIGNATURE_MARKER, ROOM),
    );
  });

  it('is idempotent — applying twice leaves ONE block', () => {
    // The whole reason the marker exists. Without it a second insert would
    // stack another copy under the first, and nothing could ever remove it.
    const once = applySignature('Agenda folgt.', ROOM);
    expect(applySignature(once, ROOM)).toBe(once);
  });

  it('REPLACES the block when the signature changes', () => {
    const first = applySignature('Agenda folgt.', ROOM);
    const second = applySignature(first, 'Vorlesung, Raum 42');
    expect(signatureIn(second)).toBe('Vorlesung, Raum 42');
    expect(second).not.toContain('conf.dfn.de');
    // …and the appointment's own text is untouched.
    expect(second.startsWith('Agenda folgt.')).toBe(true);
  });

  it('never rewrites the description itself', () => {
    const text = lines('Zeile eins', '', 'Zeile drei mit -- Gedankenstrich');
    expect(stripSignature(applySignature(text, ROOM))).toBe(text);
  });

  it('handles an empty description without a leading gap', () => {
    expect(applySignature('', ROOM)).toBe(lines(SIGNATURE_MARKER, ROOM));
    expect(applySignature('   ', ROOM)).toBe(lines(SIGNATURE_MARKER, ROOM));
  });

  it('removes the block for an empty signature', () => {
    const withSig = applySignature('Agenda folgt.', ROOM);
    expect(applySignature(withSig, '')).toBe('Agenda folgt.');
    expect(applySignature(withSig, '   ')).toBe('Agenda folgt.');
  });

  it('does not grow a gap when a signature is swapped repeatedly', () => {
    let out = applySignature('Text.', ROOM);
    for (let i = 0; i < 5; i += 1) out = applySignature(out, ROOM);
    expect(out).toBe(lines('Text.', '', SIGNATURE_MARKER, ROOM));
  });
});

describe('stripSignature / signatureIn', () => {
  it('reads back what was applied', () => {
    expect(signatureIn(applySignature('Agenda.', ROOM))).toBe(ROOM);
  });

  it('says null when there is no block', () => {
    expect(signatureIn('Nur Text.')).toBeNull();
    expect(stripSignature('Nur Text.')).toBe('Nur Text.');
  });

  it('takes the LAST marker, so a quoted one above is left alone', () => {
    // A forwarded invitation can carry somebody else's signature in its body.
    // Ours is the one at the end — the same rule mail clients apply.
    const text = lines(
      'Weitergeleitet:',
      SIGNATURE_MARKER,
      'Die Signatur der Kollegin',
      '',
      SIGNATURE_MARKER,
      'Meine',
    );
    expect(signatureIn(text)).toBe('Meine');
    expect(stripSignature(text)).toBe(
      lines('Weitergeleitet:', SIGNATURE_MARKER, 'Die Signatur der Kollegin'),
    );
  });

  it('recognises a marker whose trailing space a provider ate', () => {
    // Round-tripping through a provider can trim line ends. The convention is
    // "dash dash space", but a block that comes back as "--" is still ours,
    // and failing to see it would mean appending a second one.
    const text = lines('Agenda.', '', '--', ROOM);
    expect(signatureIn(text)).toBe(ROOM);
    expect(applySignature(text, ROOM)).toBe(
      lines('Agenda.', '', SIGNATURE_MARKER, ROOM),
    );
  });
});
