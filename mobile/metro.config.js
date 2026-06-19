// Learn more: https://docs.expo.dev/guides/customizing-metro/
const { getDefaultConfig } = require('expo/metro-config');
const path = require('path');

const config = getDefaultConfig(__dirname);

// @aperio/locales + @aperio/shared live at the repo root, OUTSIDE this Expo
// project, and are consumed as local `file:` dependencies (see package.json):
// `npm install` symlinks them into node_modules, so Metro resolves them as
// ordinary packages. That works in the dev server AND the production
// `expo export` / EAS bundle — unlike a `watchFolders` + `extraNodeModules`
// alias, which the production resolver did NOT honour for deep JSON imports like
// `@aperio/locales/de/translation.json` (it resolved only in the dev server, so
// the gap surfaced as a release-bundle failure on EAS). We deliberately keep
// them as plain file: deps rather than an npm workspace — the desktop pins React
// 18 and mobile React 19, and these two packages are dependency-free, so there's
// nothing to hoist or clash.
//
// We still watch the real folders so edits to the shared code hot-reload in dev.
const localesDir = path.resolve(__dirname, '..', 'locales');
const sharedDir = path.resolve(__dirname, '..', 'shared');
config.watchFolders = [...(config.watchFolders ?? []), localesDir, sharedDir];

module.exports = config;
