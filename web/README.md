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
Pages on every push to `web/**`. Interim target: the project path
`https://timtam.github.io/aperio/` (so `base` is `/aperio/`).

### Switching to a custom domain (for OAuth verification)

The domain is needed for Google/Microsoft verification anyway. To move from the
`/aperio/` project path to a domain root:

1. Add the domain as a GitHub Pages custom domain (creates a `CNAME`).
2. In `astro.config.mjs`, set `SITE` to the domain and `BASE` to `'/'`. The
   rehype plugin re-bases content links and Starlight re-bases its own nav
   automatically.
3. Drop the `/aperio` prefix from the **hero `link:` values** in
   `src/content/docs/index.mdx` and `src/content/docs/de/index.mdx` (hero links
   come from frontmatter and bypass the rehype re-basing — the only manual
   touch-point).
4. Fill in the real legal content (`/impressum`, `/privacy`, `/terms` + `/de/…`)
   and have it reviewed.
