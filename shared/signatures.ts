/** Signature blocks for event and task descriptions — this surface's door
 *  into `cal_core::signatures`.
 *
 *  The recurring work around a conference room is not creating it — a DFNconf
 *  room is permanent, and most people hold two or three. It is getting the
 *  room's join details into every appointment. That generalises: a lecturer's
 *  consultation hours, a department's dial-in, a standing "please bring your
 *  own laptop" — all the same shape, and all the same shape mail clients solved
 *  decades ago with signatures bound to accounts.
 *
 *  ## Why plain text, and only plain text
 *
 *  A sent invitation is an iCalendar object, and RFC 5545 §3.8.1.5 gives
 *  DESCRIPTION the TEXT value type: no markup, only the escapes `\\n`, `\\,`,
 *  `\\;`, `\\\\`. HTML put there is text that happens to look like tags, and it
 *  reaches every recipient whose client renders the description literally as
 *  exactly that — which is worse with a screen reader than plain prose, not
 *  better.
 *
 *  The two escape hatches are both dead ends for storage.
 *  `X-ALT-DESC;FMTTYPE=text/html` is a Microsoft extension that Outlook 2013
 *  used, Outlook 2016 dropped, and Google Calendar ignores in favour of
 *  DESCRIPTION. RFC 9073's STYLED-DESCRIPTION is the standards-track answer and
 *  is barely deployed. If a rich form is ever added it will be an ADDITION
 *  emitted beside the text — never the thing we store, because plain text is
 *  the only form guaranteed to arrive.
 *
 *  So: a URL on a line of its own (which practically every client linkifies)
 *  and blank lines for structure. That is the whole formatting vocabulary, and
 *  it survives everywhere.
 *
 *  ## Where the rule lives
 *
 *  Where a block starts, what it says, and how a body is applied is decided in
 *  the core now (`cal_core::signatures`), so two devices write the same bytes
 *  and a frontend that is not JavaScript writes them too. This file is the
 *  door: the three functions keep their names and hand the strings through.
 *  Pinned by `crates/cal-core/tests/fixtures/signatures.json`, measured from
 *  the TypeScript this replaced; `signatures.contract.test.ts` replays it
 *  through this door, and the core's own contract test reads the same file.
 */

import type { ApplySignatureInput, SignatureTextInput } from './types';

/**
 * The separator that opens a signature block.
 *
 * Borrowed from mail (RFC 3676 §4.3): a line containing exactly `-- `, dash
 * dash space. It earns its place twice — readers who know it recognise what
 * follows as a signature rather than as part of the appointment, and it gives
 * Aperio something to find, so changing a signature REPLACES the old block
 * instead of stacking another copy underneath it.
 *
 * The trailing space is part of the convention and is deliberately preserved.
 * The core writes it (`cal_core::signatures::SIGNATURE_MARKER`); this copy is
 * for the tests that spell a block out, and the contract test checks the two
 * agree.
 */
export const SIGNATURE_MARKER = '-- ';

export interface Signature {
  id: string;
  /** What the user calls it — "Sprechstunde", "Vorlesung". Never sent. */
  name: string;
  /** The text itself, sent verbatim. */
  body: string;
}

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::signatures`. */
export interface SignatureRules {
  signatureInJson(inputJson: string): string;
  stripSignatureJson(inputJson: string): string;
  applySignatureJson(inputJson: string): string;
}

let installedRules: SignatureRules | null = null;

/** Bind this surface's door into the core. */
export function installSignatureRules(rules: SignatureRules): void {
  installedRules = rules;
}

function rules(): SignatureRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the copy all over
    // again, and its failure is the one the marker exists to prevent: a block
    // one device inserted and the other one doubled.
    throw new Error(
      'signature rules used before installSignatureRules() — the surface must ' +
        'bind its door into cal_core::signatures at startup',
    );
  }
  return installedRules;
}

/** The description without its signature block, and without the blank line
 *  that separated them. */
export function stripSignature(description: string): string {
  const input: SignatureTextInput = { description };
  return JSON.parse(rules().stripSignatureJson(JSON.stringify(input))) as string;
}

/** What the description's signature block says, or `null` when it has none. */
export function signatureIn(description: string): string | null {
  const input: SignatureTextInput = { description };
  return JSON.parse(rules().signatureInJson(JSON.stringify(input))) as string | null;
}

/**
 * The description with `body` as its signature block.
 *
 * Replaces an existing block rather than appending a second one, so applying
 * twice — or switching from one signature to another — leaves exactly one. An
 * empty body removes the block entirely.
 *
 * The appointment's own text is never touched: a signature is an addition at
 * the end, not a rewrite.
 */
export function applySignature(description: string, body: string): string {
  // The editors hand over `undefined` for a description that was never set;
  // the TypeScript read that as an empty text, and so does the door.
  const input: ApplySignatureInput = { description: description ?? '', body };
  return JSON.parse(rules().applySignatureJson(JSON.stringify(input))) as string;
}
