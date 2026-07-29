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
    out.push({ url, text: m.text });
  }
  return out;
}
