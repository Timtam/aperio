// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Deploy target. Interim: GitHub Pages *project* site at /aperio/. When the
// custom domain lands, set `site` to it and `BASE` to '/' — that single
// change is enough; the rehype plugin below re-bases content links and
// Starlight re-bases its own navigation automatically.
const SITE = 'https://timtam.github.io';
const BASE = '/aperio/';

/**
 * Prefix internal absolute links (`/guides/…`) in Markdown/MDX content with
 * the deploy base. Starlight already bases its own nav/sidebar/links, but
 * raw author-written absolute hrefs in prose are not touched by Astro — so a
 * `/guides/x` link would 404 under a project base without this. Idempotent:
 * already-based and external (`//`, `http`) links are left alone.
 */
function rehypeBaseLinks() {
  const prefix = BASE.replace(/\/$/, ''); // "/aperio"
  /** @param {any} node */
  const walk = (node) => {
    if (node.type === 'element' && node.tagName === 'a') {
      const href = node.properties && node.properties.href;
      if (
        typeof href === 'string' &&
        href.startsWith('/') &&
        !href.startsWith('//') &&
        href !== prefix &&
        !href.startsWith(prefix + '/')
      ) {
        node.properties.href = prefix + href;
      }
    }
    if (node.children) node.children.forEach(walk);
  };
  return (/** @type {any} */ tree) => walk(tree);
}

/**
 * Chapters that moved when Contacts became chapter 05 (everything from the old
 * 05 onwards shifted by one), plus the short-lived Reference page it replaced.
 *
 * These are not cosmetic: a tutorial chapter is exactly the kind of URL people
 * bookmark and link to, and a static host answers a stale one with a bare 404.
 * Astro emits a meta-refresh page per entry on a static build, which is what
 * GitHub Pages can serve. Keep them — an old link staying alive costs one
 * generated file each.
 *
 * Written base-relative and prefixed below: Astro bases the SOURCE of a
 * redirect but NOT its destination, so an unprefixed target sends the reader
 * from /aperio/guides/… to /guides/… — off the site and into a 404, which is
 * exactly the failure these entries exist to prevent.
 */
const MOVED_PAGES = Object.fromEntries(
  Object.entries({
    '/guides/kontakte': '/guides/tutorial/05-kontakte',
    '/de/guides/kontakte': '/de/guides/tutorial/05-kontakte',
    '/guides/tutorial/05-ansichten': '/guides/tutorial/06-ansichten',
    '/de/guides/tutorial/05-ansichten': '/de/guides/tutorial/06-ansichten',
    '/guides/tutorial/06-benachrichtigungen': '/guides/tutorial/07-benachrichtigungen',
    '/de/guides/tutorial/06-benachrichtigungen': '/de/guides/tutorial/07-benachrichtigungen',
    '/guides/tutorial/07-suche': '/guides/tutorial/08-suche',
    '/de/guides/tutorial/07-suche': '/de/guides/tutorial/08-suche',
    '/guides/tutorial/08-synchronisation': '/guides/tutorial/09-synchronisation',
    '/de/guides/tutorial/08-synchronisation': '/de/guides/tutorial/09-synchronisation',
    '/guides/tutorial/09-tastaturkuerzel': '/guides/tutorial/10-tastaturkuerzel',
    '/de/guides/tutorial/09-tastaturkuerzel': '/de/guides/tutorial/10-tastaturkuerzel',
  }).map(([from, to]) => [from, `${BASE.replace(/\/$/, '')}${to}/`]),
);

export default defineConfig({
  site: SITE,
  base: BASE,
  redirects: MOVED_PAGES,
  markdown: { rehypePlugins: [rehypeBaseLinks] },
  integrations: [
    starlight({
      title: 'Aperio',
      tagline: 'Accessible, keyboard-first calendar, tasks & contacts.',
      defaultLocale: 'root',
      locales: {
        root: { label: 'English', lang: 'en' },
        de: { label: 'Deutsch', lang: 'de' },
      },
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/Timtam/aperio',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/Timtam/aperio/edit/main/web/',
      },
      sidebar: [
        {
          label: 'User Guide',
          translations: { de: 'Benutzerhandbuch' },
          items: [
            {
              label: 'Welcome',
              translations: { de: 'Willkommen' },
              slug: 'guides',
            },
            {
              label: 'Tutorial',
              translations: { de: 'Tutorial' },
              items: [
                { label: '01 – Installation & First Launch', translations: { de: '01 – Installation & Start' }, slug: 'guides/tutorial/01-installation' },
                { label: '02 – Connecting Calendars and Task Lists', translations: { de: '02 – Kalender und Aufgabenlisten verbinden' }, slug: 'guides/tutorial/02-konten-verbinden' },
                { label: '03 – Events', translations: { de: '03 – Termine' }, slug: 'guides/tutorial/03-termine' },
                { label: '04 – Tasks', translations: { de: '04 – Aufgaben' }, slug: 'guides/tutorial/04-aufgaben' },
                { label: '05 – Contacts', translations: { de: '05 – Kontakte' }, slug: 'guides/tutorial/05-kontakte' },
                { label: '06 – Views', translations: { de: '06 – Ansichten' }, slug: 'guides/tutorial/06-ansichten' },
                { label: '07 – Notifications', translations: { de: '07 – Benachrichtigungen' }, slug: 'guides/tutorial/07-benachrichtigungen' },
                { label: '08 – Search', translations: { de: '08 – Suche' }, slug: 'guides/tutorial/08-suche' },
                { label: '09 – Synchronization', translations: { de: '09 – Synchronisation' }, slug: 'guides/tutorial/09-synchronisation' },
                { label: '10 – Keyboard Shortcuts', translations: { de: '10 – Tastaturkürzel' }, slug: 'guides/tutorial/10-tastaturkuerzel' },
              ],
            },
            {
              label: 'Reference',
              translations: { de: 'Referenz' },
              items: [
                { label: 'Mobile app', translations: { de: 'Mobile App' }, slug: 'guides/mobile' },
                { label: 'Keyboard Shortcuts', translations: { de: 'Tastaturkürzel' }, slug: 'guides/tastaturkuerzel' },
                { label: 'Accessibility', translations: { de: 'Barrierefreiheit' }, slug: 'guides/barrierefreiheit' },
                { label: 'Troubleshooting & Logs', translations: { de: 'Fehlersuche & Protokolle' }, slug: 'guides/troubleshooting' },
                { label: 'Connecting Google (OAuth guide)', translations: { de: 'Google einbinden (OAuth-Anleitung)' }, slug: 'guides/google-oauth' },
                { label: 'Video meetings with Webex', translations: { de: 'Videokonferenzen mit Webex' }, slug: 'guides/webex' },
              ],
            },
          ],
        },
        {
          label: 'Developer Documentation',
          translations: { de: 'Entwickler-Dokumentation' },
          items: [
            { label: 'Introduction', slug: 'developers' },
            { label: 'Getting Started', slug: 'developers/getting-started' },
            { label: 'Architecture', slug: 'developers/architecture' },
            { label: 'Contributing', slug: 'developers/contributing' },
            { label: 'Testing', slug: 'developers/testing' },
            {
              label: 'Adapters',
              items: [
                { label: 'Overview', slug: 'developers/adapters/overview' },
                { label: 'Local store', slug: 'developers/adapters/local' },
                { label: 'CalDAV / iCloud', slug: 'developers/adapters/caldav' },
                { label: 'Google', slug: 'developers/adapters/google' },
                { label: 'Microsoft Graph', slug: 'developers/adapters/microsoft' },
                { label: 'Exchange (EWS)', slug: 'developers/adapters/ews' },
                { label: 'Vikunja', slug: 'developers/adapters/vikunja' },
                { label: 'Todoist', slug: 'developers/adapters/todoist' },
              ],
            },
          ],
        },
        {
          label: 'Plugin Development',
          translations: { de: 'Plugin-Entwicklung' },
          items: [
            { label: 'Introduction', slug: 'plugins' },
            { label: 'Getting Started', slug: 'plugins/getting-started' },
            { label: 'The C ABI', slug: 'plugins/abi-reference' },
            { label: 'ABI versions + migration', slug: 'plugins/abi-versions' },
            { label: 'The Rust SDK', slug: 'plugins/rust-sdk' },
            { label: 'The plugin.json manifest', slug: 'plugins/manifest' },
            {
              label: 'Examples',
              items: [
                { label: 'Overview', slug: 'plugins/examples' },
                { label: 'hello-world', slug: 'plugins/examples/hello-world' },
                { label: 'calendar-adapter-template', slug: 'plugins/examples/calendar-adapter-template' },
              ],
            },
          ],
        },
      ],
    }),
  ],
});
