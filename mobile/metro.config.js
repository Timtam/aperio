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
config.watchFolders = [...(config.watchFolders ?? []), localesDir];
config.resolver.extraNodeModules = {
  ...config.resolver.extraNodeModules,
  '@aperio/locales': localesDir,
};

module.exports = config;
