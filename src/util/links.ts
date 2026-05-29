// URL detection for plain-text descriptions.
//
// Event / task descriptions are plain text (EWS bodies are fetched as
// `BodyType=Text`; the local store keeps a plain string). To make the
// links inside them clickable we detect URLs with `linkify-it` — the
// same battle-tested matcher markdown-it uses — then keep only the
// schemes we're willing to hand to the OS opener.
//
// Security note: detection here is purely for UX. The real gate is the
// `open_external_url` Rust command, which re-validates the scheme. We
// still filter to http/https/mailto here so the link bar never offers
// something the backend would refuse.

import LinkifyIt from 'linkify-it';

export interface DetectedLink {
  /** Fully-qualified, normalised URL handed to the opener
   *  (e.g. `www.x.com` → `http://www.x.com`, an address →
   *  `mailto:user@x.com`). This is also what we show, so the user
   *  sees exactly where the link goes. */
  url: string;
  /** The raw text as it appears in the description — used as the
   *  accessible label / fallback display when slightly friendlier
   *  than the normalised URL. */
  text: string;
}

/** Schemes we allow to reach the OS handler. Mirrors the allowlist in
 *  `src-tauri/src/commands/external.rs`. */
const ALLOWED_SCHEMES = new Set(['http:', 'https:', 'mailto:']);

// One shared instance — linkify-it is stateless across `match` calls.
// Defaults already enable fuzzy link + fuzzy email detection, so bare
// `www.example.com` and `user@example.com` are picked up.
const linkify = new LinkifyIt();

function schemeOf(url: string): string | null {
  const idx = url.indexOf(':');
  if (idx <= 0) return null;
  return url.slice(0, idx + 1).toLowerCase();
}

/**
 * Detect the openable links in `text`, in order of appearance, with
 * duplicates (same normalised URL) collapsed to the first occurrence.
 * Only http/https/mailto survive; anything else linkify-it might
 * surface is dropped.
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
    if (!scheme || !ALLOWED_SCHEMES.has(scheme)) continue;
    if (seen.has(url)) continue;
    seen.add(url);
    out.push({ url, text: m.text });
  }
  return out;
}
