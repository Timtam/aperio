/** Signature blocks for event and task descriptions.
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
 */

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
 */
export const SIGNATURE_MARKER = '-- ';

export interface Signature {
  id: string;
  /** What the user calls it — "Sprechstunde", "Vorlesung". Never sent. */
  name: string;
  /** The text itself, sent verbatim. */
  body: string;
}

/** Index of the line that opens the signature block, or -1.
 *
 *  The LAST such line wins: a description may legitimately contain the marker
 *  in quoted text above, and the block that matters is the one at the end —
 *  the same rule mail clients apply. */
function markerLine(lines: readonly string[]): number {
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    // Trailing whitespace varies once a description has been through a
    // provider; the marker is recognised by its content, not its bytes.
    if (lines[i].trimEnd() === SIGNATURE_MARKER.trimEnd()) return i;
  }
  return -1;
}

/** The description without its signature block, and without the blank line
 *  that separated them. */
export function stripSignature(description: string): string {
  const lines = description.split('\n');
  const at = markerLine(lines);
  if (at < 0) return description;
  let end = at;
  // Eat the blank line the block was separated by, so removing and re-adding
  // a signature does not grow a gap each time.
  while (end > 0 && lines[end - 1].trim() === '') end -= 1;
  return lines.slice(0, end).join('\n');
}

/** What the description's signature block says, or `null` when it has none. */
export function signatureIn(description: string): string | null {
  const lines = description.split('\n');
  const at = markerLine(lines);
  return at < 0 ? null : lines.slice(at + 1).join('\n');
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
  const stripped = stripSignature(description ?? '');
  // A description of nothing but whitespace is a description of nothing:
  // keeping it would open the block on a line of stray spaces.
  const base = stripped.trim() === '' ? '' : stripped;
  const trimmed = body.trim();
  if (trimmed === '') return base;
  const separator = base === '' ? '' : '\n\n';
  return `${base}${separator}${SIGNATURE_MARKER}\n${trimmed}`;
}
