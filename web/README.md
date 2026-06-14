# Aperio website + documentation (Astro Starlight)

This is the unified marketing site **and** documentation, replacing the four
separate mdBooks under `../docs`. One project provides:

- a landing page (`/`, German at `/de/`) — the homepage OAuth verification
  (Google, Microsoft) requires,
- legal pages (`/privacy`, `/terms`, `/impressum`, plus `/de/…`) — currently
  **placeholders** that must be filled with real details and reviewed,
- the documentation, bilingual where it applies:
  - **User Guide** — English (`/guides/…`) + German (`/de/guides/…`),
  - **Developer Documentation** — English (`/developers/…`),
  - **Plugin Development** — English (`/plugins/…`).

i18n is native (Starlight): `en` is the root locale, `de` the secondary one.
Pages without a German translation (developer/plugin docs) fall back to English
automatically. Search is Pagefind (runs in the browser).

## Develop

```bash
cd web
npm install
npm run dev      # http://localhost:4321
npm run build    # static output in dist/
npm run preview  # serve the built dist/
```

## How the content was migrated

`scripts/migrate-docs.mjs` is a one-shot transformer: it copied every mdBook
page from `../docs/{user-en,user,dev,plugin-dev}/src` into
`src/content/docs/…`, adding a `title` frontmatter from the first H1 (and
removing that H1), renaming each book's intro page to the area index, and
rewriting internal `.md` links to locale-aware Starlight URLs. It can be re-run
until the old mdBooks are decommissioned; after that it can be deleted.

## Go-live checklist (not done yet — intentionally)

The old mdBook deploy (`.github/workflows/docs.yml`) is still live and
untouched, so nothing breaks. To switch over:

1. **Register the domain** (needed for OAuth verification anyway) and add a
   `CNAME` (GitHub Pages custom domain) → the site serves from the root, so the
   absolute links (`/guides/…`) resolve correctly. _Do not_ deploy to the
   `…github.io/aperio/` project path without setting `base: '/aperio/'` and
   adjusting links.
2. Set `site: 'https://your-domain'` in `astro.config.mjs` (enables canonical
   URLs, OG tags and the sitemap).
3. Replace the `mdbook build`/assemble steps in `docs.yml` with an Astro build:
   ```yaml
   - uses: actions/setup-node@v4
     with: { node-version: 20, cache: npm, cache-dependency-path: web/package-lock.json }
   - run: npm ci
     working-directory: web
   - run: npm run build
     working-directory: web
   - uses: actions/upload-pages-artifact@v3
     with: { path: web/dist }
   ```
4. Fill in the real legal content (`/impressum`, `/privacy`, `/terms` + `/de/…`)
   and have it reviewed.
5. Once verified live, remove `../docs` (the four mdBooks) and this migration
   script.
