// URL detection for plain-text descriptions (shared desktop + mobile).
//
// Event / task descriptions are plain text (EWS bodies are fetched as
// `BodyType=Text`; the local store keeps a plain string). To make the links
// inside them activatable we detect URLs with `linkify-it` — the same matcher
// markdown-it uses — then keep only the schemes we're willing to hand to the OS
// opener.
//
// Security note: detection here is purely for UX. The real gate is the opener —
// the desktop `open_external_url` Rust command, and on mobile a scheme check
// before `Linking.openURL`. We still filter to http/https/mailto here so the
// link bar never offers something the opener would refuse.

import LinkifyIt from 'linkify-it';

export interface DetectedLink {
  /** Fully-qualified, normalised URL handed to the opener (e.g. `www.x.com` →
   *  `http://www.x.com`, an address → `mailto:user@x.com`). This is also what we
   *  show, so the user sees exactly where the link goes. */
  url: string;
  /** The raw text as it appears in the description — used as the accessible
   *  label / fallback display when slightly friendlier than the normalised URL. */
  text: string;
  /**
   * What the description itself calls this link, when it says.
   *
   * Real invitations — Aperio's own join block, and the ones Outlook, eM Client
   * and Webex write — put each fact on a `Label: value` line. When a link is
   * the whole value of such a line, that label is what the link IS, and it is
   * what a reader should hear: "Join the meeting" rather than ninety
   * characters of query string. Treated as data, exactly as `cal_core`'s
   * conference detection treats the same labels, so it works in whatever
   * language the invitation arrived in.
   *
   * Absent for a link written mid-sentence, which has no name to take.
   */
  label?: string;
}

/** Schemes allowed to reach the OS handler. Mirrors the desktop allowlist in
 *  `src-tauri/src/commands/external.rs` AND the mobile open-link guard. */
export const ALLOWED_LINK_SCHEMES = new Set([
  'http:',
  'https:',
  'mailto:',
  // A meeting block writes dial-in numbers and a video-system address. Without
  // these the link bar would refuse to offer the very number Aperio had just
  // written into the event — which matters most for the person who joins by
  // phone rather than by clicking.
  'tel:',
  'sip:',
  'sips:',
]);

// One shared instance — linkify-it is stateless across `match` calls. Defaults
// already enable fuzzy link + fuzzy email detection, so bare `www.example.com`
// and `user@example.com` are picked up.
const linkify = new LinkifyIt();

/** The scheme of `url` (incl. the trailing colon), lower-cased, or null. */
export function schemeOf(url: string): string | null {
  const idx = url.indexOf(':');
  if (idx <= 0) return null;
  return url.slice(0, idx + 1).toLowerCase();
}

/**
 * Detect the openable links in `text`, in order of appearance, with duplicates
 * (same normalised URL) collapsed to the first occurrence. Only http/https/
 * mailto survive; anything else linkify-it might surface is dropped.
 */
export function detectLinks(text: string | null | undefined): DetectedLink[] {
  if (!text) return [];
  const matches = linkify.match(text);
  if (!matches) return [];

  const seen = new Set<string>();
  const out: DetectedLink[] = [];
  for (const m of matches) {
    const url = m.url;
    const scheme = schemeOf(url);
    if (!scheme || !ALLOWED_LINK_SCHEMES.has(scheme)) continue;
    if (seen.has(url)) continue;
    seen.add(url);
    out.push({ url, text: m.text, label: labelOf(text, m.index, m.lastIndex) });
  }
  return out;
}

/** How long a run of text before the colon may be and still be a label. */
const MAX_LABEL = 40;

/**
 * The label naming the link that starts at `index`, if its line is
 * `Label: <link>` and the link is the whole of the value.
 *
 * Deliberately narrow. A line that merely happens to contain a colon before a
 * link — "See also, details here: https://…" reads fine, but "Bring these to
 * the meeting: https://…" would name the link "Bring these to the meeting" —
 * is still a fair name for it, so the only real guards are length and that
 * nothing follows the link on the line. A label that turns out unhelpful costs
 * a reader one wrong-sounding item; the URL is still on the tile beside it.
 */
function labelOf(text: string, start: number, end: number): string | undefined {
  const lineStart = text.lastIndexOf('\n', start - 1) + 1;
  const lineEndRaw = text.indexOf('\n', start);
  const lineEnd = lineEndRaw === -1 ? text.length : lineEndRaw;
  // Anything after the link on its line means the link sits inside a sentence
  // rather than being the whole value of a labelled field.
  if (text.slice(end, lineEnd).trim() !== '') return undefined;
  const before = text.slice(lineStart, start);
  const colon = before.lastIndexOf(':');
  if (colon <= 0) return undefined;
  // Only whitespace may sit between the colon and the link.
  if (before.slice(colon + 1).trim() !== '') return undefined;
  const label = before.slice(0, colon).trim();
  if (!label || label.length > MAX_LABEL) return undefined;
  // A bare URL splits into `https` + `//host`; that is a scheme, not a label.
  if (label.includes('://') || /^[a-z][a-z0-9+.-]*$/i.test(label)) {
    return undefined;
  }
  return label;
}
