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

This site replaced the four mdBooks that used to live under `docs/` (now
removed). It is the single source for the landing page, legal pages and all
documentation.

## Deployment

`.github/workflows/docs.yml` builds **this** site and deploys it to GitHub
Pages on every push to `web/**`, at the root of Aperio's own domain:
**<https://aperiocal.app/>** (since 2026-11-17; `base` is `/`).

### The domain

- The custom domain is set in the repository's Pages settings, not by a
  `CNAME` file: a site deployed by a workflow ignores one. The domain is
  verified for the GitHub account (a `_github-pages-challenge-…` TXT record),
  and GitHub issues its certificate; HTTPS is enforced (`.app` works only
  over HTTPS anyway).
- DNS (at the registrar, netcup): four `A` and four `AAAA` records for the
  apex pointing at GitHub Pages, `www` as a `CNAME` to `timtam.github.io`, no
  wildcard record.
- The site used to be the project site at `timtam.github.io/aperio/`. GitHub
  redirects that to the domain with the path kept, so an old link arrives at
  `aperiocal.app/aperio/…`. Every page carries a small script
  (`OLD_BASE_REDIRECT` in `astro.config.mjs`) that drops that prefix; the
  404 page is the one that runs it.
- Hero links (`src/content/docs/index.mdx`, `src/content/docs/de/index.mdx`)
  come from frontmatter and bypass the rehype re-basing: should `BASE` ever
  change, change them by hand.
- Still open: the terms page (`/terms`, `/de/terms`) is a draft with
  placeholders.
