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

export default defineConfig({
  site: SITE,
  base: BASE,
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
                { label: '05 – Views', translations: { de: '05 – Ansichten' }, slug: 'guides/tutorial/05-ansichten' },
                { label: '06 – Notifications', translations: { de: '06 – Benachrichtigungen' }, slug: 'guides/tutorial/06-benachrichtigungen' },
                { label: '07 – Search', translations: { de: '07 – Suche' }, slug: 'guides/tutorial/07-suche' },
                { label: '08 – Synchronization', translations: { de: '08 – Synchronisation' }, slug: 'guides/tutorial/08-synchronisation' },
                { label: '09 – Keyboard Shortcuts', translations: { de: '09 – Tastaturkürzel' }, slug: 'guides/tutorial/09-tastaturkuerzel' },
              ],
            },
            {
              label: 'Reference',
              translations: { de: 'Referenz' },
              items: [
                { label: 'Keyboard Shortcuts', translations: { de: 'Tastaturkürzel' }, slug: 'guides/tastaturkuerzel' },
                { label: 'Accessibility', translations: { de: 'Barrierefreiheit' }, slug: 'guides/barrierefreiheit' },
                { label: 'Connecting Google (OAuth guide)', translations: { de: 'Google einbinden (OAuth-Anleitung)' }, slug: 'guides/google-oauth' },
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
