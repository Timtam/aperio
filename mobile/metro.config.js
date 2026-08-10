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
// 18 and mobile React 19, so hoisting them into one tree would clash.
//
// @aperio/shared is NOT dependency-free (it imports rrule and linkify-it); that
// is what the resolver root below is for. @aperio/locales is pure JSON.
//
// We still watch the real folders so edits to the shared code hot-reload in dev.
const localesDir = path.resolve(__dirname, '..', 'locales');
const sharedDir = path.resolve(__dirname, '..', 'shared');
config.watchFolders = [...(config.watchFolders ?? []), localesDir, sharedDir];

// @aperio/shared's own files (e.g. shared/links.ts → `linkify-it`, recurrence.ts
// → `rrule`) import third-party packages that are declared in THIS app's
// package.json. Because the package is symlinked at the repo root, Metro's
// hierarchical lookup from shared/*.ts would search the repo-root node_modules
// (absent on EAS — only mobile/node_modules is installed), so those imports
// failed the production bundle. Add this project's node_modules as an explicit
// resolver root so the shared package resolves its deps from where they're
// actually installed — the standard Expo-monorepo nodeModulesPaths fix.
config.resolver.nodeModulesPaths = [
  ...(config.resolver.nodeModulesPaths ?? []),
  path.resolve(__dirname, 'node_modules'),
];

module.exports = config;
