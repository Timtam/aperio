// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// TODO: set `site` to the production domain once registered (needed for
// canonical URLs, OG tags and sitemap). Deployed at the domain root, so
// `base` stays "/". For a GitHub Pages *project* path instead, set
// base: '/aperio/' and rewrite the absolute links accordingly.
export default defineConfig({
  // site: 'https://your-domain.example',
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
