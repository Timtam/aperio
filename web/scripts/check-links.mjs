// Post-build internal-link check: walk every HTML file in dist/ and verify
// each same-site href resolves to a file the build actually produced.
//
// Astro emits NO error for a dead internal link, and a static host answers it
// with a bare 404 — the failure mode that broke the tutorial renumbering
// (redirect targets missing the /aperio/ base went clean through the build).
// Checking the built output catches every variant of that at once: typoed
// hrefs, un-based redirect targets, links to renamed pages.
//
// Usage: node scripts/check-links.mjs   (after `npm run build`)

import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

const DIST = resolve(import.meta.dirname, '..', 'dist');
// Must match astro.config.mjs. The config exports nothing, so restate it here;
// a drift shows up immediately as "every link is external/unknown".
const BASE = '/aperio/';

if (!existsSync(DIST)) {
  console.error(`dist/ not found at ${DIST} — run \`npm run build\` first.`);
  process.exit(2);
}

/** @returns {string[]} every .html file under dir, recursively */
function htmlFiles(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...htmlFiles(p));
    else if (name.endsWith('.html')) out.push(p);
  }
  return out;
}

/** A same-site path resolves if the build wrote a file for it. */
function resolves(urlPath) {
  const rel = urlPath.replace(/^\/+/, '').replace(/\/+$/, '');
  const candidates = [
    join(DIST, rel),
    join(DIST, rel, 'index.html'),
    join(DIST, `${rel}.html`),
  ];
  return candidates.some((c) => existsSync(c));
}

const files = htmlFiles(DIST);
const broken = [];
const HREF = /href="([^"#?]+)[^"]*"/g;

for (const file of files) {
  const html = readFileSync(file, 'utf8');
  for (const [, href] of html.matchAll(HREF)) {
    // Only same-site absolute paths: external URLs, mailto:, and in-page
    // anchors are out of scope; relative hrefs don't occur in Starlight's
    // output.
    if (!href.startsWith('/') || href.startsWith('//')) continue;
    if (!href.startsWith(BASE) && href !== BASE.replace(/\/$/, '')) {
      broken.push({ file, href, why: 'missing deploy base' });
      continue;
    }
    const sitePath = href.slice(BASE.replace(/\/$/, '').length);
    if (sitePath !== '' && !resolves(sitePath)) {
      broken.push({ file, href, why: 'no such page in dist/' });
    }
  }
}

if (broken.length > 0) {
  console.error(`${broken.length} broken internal link(s):`);
  for (const b of broken) {
    console.error(`  ${b.file.slice(DIST.length + 1)} → ${b.href}  (${b.why})`);
  }
  process.exit(1);
}
console.log(`All internal links resolve (${files.length} pages checked).`);
