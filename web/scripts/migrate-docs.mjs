// One-shot migration: mdBook `src/*.md` → Starlight content collection.
// Adds `title` frontmatter from the first H1 (and strips that H1 to avoid a
// duplicate), renames each book's intro page to the area index, and rewrites
// internal `.md` links to root-relative Starlight URLs (locale-aware).
// Writes LF line endings. Safe to re-run: it overwrites the generated files.
import { promises as fs } from 'node:fs';
import path from 'node:path';

const DEST_ROOT = path.resolve('src/content/docs');

const books = [
  { srcDir: '../docs/user-en/src', area: 'guides', localeDir: '', intro: 'einstieg.md' },
  { srcDir: '../docs/user/src', area: 'guides', localeDir: 'de/', intro: 'einstieg.md' },
  { srcDir: '../docs/dev/src', area: 'developers', localeDir: '', intro: 'introduction.md' },
  { srcDir: '../docs/plugin-dev/src', area: 'plugins', localeDir: '', intro: 'introduction.md' },
];

async function walk(dir, baseDir, out = []) {
  for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      await walk(full, baseDir, out);
    } else if (entry.name.endsWith('.md') && entry.name !== 'SUMMARY.md') {
      out.push(path.relative(baseDir, full).split(path.sep).join('/'));
    }
  }
  return out;
}

// Map a book-relative source path (e.g. "tutorial/03-termine.md") to the
// public Starlight URL for that book + locale.
function urlFor(book, relNoExt) {
  const prefix = `/${book.localeDir}${book.area}`;
  // Intro page → area root.
  if (relNoExt + '.md' === book.intro) return prefix + '/';
  // Any other index.md → its directory.
  const slug = relNoExt.replace(/\/index$/, '').replace(/^index$/, '');
  return slug ? `${prefix}/${slug}/` : `${prefix}/`;
}

function rewriteLinks(content, book, relPath) {
  const dir = path.posix.dirname(relPath); // book-relative dir of this file
  return content.replace(/\]\(([^)]+)\)/g, (whole, href) => {
    const raw = href.trim();
    if (/^(https?:|mailto:|#|\/)/i.test(raw)) return whole; // external / anchor / already-absolute
    const [pathPart, anchor] = raw.split('#');
    if (!pathPart.endsWith('.md')) return whole; // not an internal doc link
    let resolved = path.posix.normalize(path.posix.join(dir, pathPart));
    if (resolved.startsWith('..')) {
      console.warn(`  ! link escapes book in ${book.area}/${relPath}: ${raw}`);
      return whole;
    }
    const url = urlFor(book, resolved.replace(/\.md$/, ''));
    return `](${anchor ? url + '#' + anchor : url})`;
  });
}

function transform(content, book, relPath) {
  let body = content.replace(/\r\n/g, '\n');
  // Pull the first H1 as the page title, then drop that line.
  const m = body.match(/^#\s+(.+?)\s*$/m);
  const title = m ? m[1].trim() : path.posix.basename(relPath, '.md');
  if (m) {
    body = body.replace(m[0], '').replace(/^\n+/, '');
  }
  body = rewriteLinks(body, book, relPath);
  const fm = `---\ntitle: ${JSON.stringify(title)}\n---\n\n`;
  return fm + body.trimStart();
}

async function run() {
  let count = 0;
  for (const book of books) {
    const srcAbs = path.resolve(book.srcDir);
    const files = await walk(srcAbs, srcAbs);
    for (const rel of files) {
      const content = await fs.readFile(path.join(srcAbs, rel), 'utf8');
      const out = transform(content, book, rel);
      // Intro page becomes the area index; everything else keeps its path.
      const destRel =
        rel === book.intro
          ? `${book.localeDir}${book.area}/index.md`
          : `${book.localeDir}${book.area}/${rel}`;
      const destAbs = path.join(DEST_ROOT, destRel);
      await fs.mkdir(path.dirname(destAbs), { recursive: true });
      await fs.writeFile(destAbs, out, 'utf8');
      count++;
    }
    console.log(`✓ ${book.srcDir} → ${book.localeDir}${book.area} (${files.length} pages)`);
  }
  console.log(`Migrated ${count} pages.`);
}

run().catch((e) => {
  console.error(e);
  process.exit(1);
});
