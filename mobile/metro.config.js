// Learn more: https://docs.expo.dev/guides/customizing-metro/
const { getDefaultConfig } = require('expo/metro-config');
const path = require('path');

const config = getDefaultConfig(__dirname);

// Share the dep-free @aperio/locales package, which lives at the repo root —
// OUTSIDE this Expo project. We deliberately do NOT fold mobile into an npm
// workspace: the desktop frontend pins React 18 and mobile pins React 19, so a
// hoisted workspace would clash. Instead we alias just the locale JSON files
// (no dependencies of their own) and point Metro at that one folder so it can
// read and watch it. Deep imports like `@aperio/locales/de/translation.json`
// resolve through the alias.
const localesDir = path.resolve(__dirname, '..', 'locales');

// The shared, platform-agnostic frontend domain (@aperio/shared) lives at the
// repo root too — the task types + grouping + label helpers reused 1:1 with the
// desktop. Same alias mechanism as @aperio/locales (NOT an npm workspace), so
// `@aperio/shared` resolves to shared/index.ts and Metro watches the folder.
const sharedDir = path.resolve(__dirname, '..', 'shared');
config.watchFolders = [...(config.watchFolders ?? []), localesDir, sharedDir];
config.resolver.extraNodeModules = {
  ...config.resolver.extraNodeModules,
  '@aperio/locales': localesDir,
  '@aperio/shared': sharedDir,
};

module.exports = config;
