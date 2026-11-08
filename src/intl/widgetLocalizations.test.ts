// The widget extension has to declare the languages the app speaks. iOS draws
// the Live Activity's self-updating times ("in 59 minutes") by itself, in the
// extension's own language, and an extension that declares none speaks the
// development language: English on a German phone (2026-11-08). The app's
// list lives in app.json (expo-localization), the extension's in its own
// Info.plist; nothing but this test keeps the two in step.

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const ROOT = resolve(__dirname, '../..');
const read = (relative: string) => readFileSync(resolve(ROOT, relative), 'utf8');

/** The languages app.json hands expo-localization. */
function appLocales(): string[] {
  const app = JSON.parse(read('mobile/app.json')) as {
    expo: { plugins: unknown[] };
  };
  const entry = app.expo.plugins.find(
    (plugin): plugin is [string, { supportedLocales: string[] }] =>
      Array.isArray(plugin) && plugin[0] === 'expo-localization',
  );
  return entry ? [...entry[1].supportedLocales].sort() : [];
}

/** The `<string>`s of the array that follows `<key>CFBundleLocalizations</key>`. */
function extensionLocales(): string[] {
  const plist = read('mobile/targets/widget/Info.plist');
  const array = /<key>CFBundleLocalizations<\/key>\s*<array>([\s\S]*?)<\/array>/.exec(plist);
  if (!array) return [];
  return [...array[1].matchAll(/<string>([^<]+)<\/string>/g)].map((m) => m[1]).sort();
}

describe('widget extension languages', () => {
  it('are the languages the app declares', () => {
    expect(appLocales().length).toBeGreaterThan(0);
    expect(extensionLocales()).toEqual(appLocales());
  });

  it('keep the widget point the plugin would have written', () => {
    // The plugin writes this file only when it is missing; once it is
    // committed, nothing else says what kind of extension this is.
    expect(read('mobile/targets/widget/Info.plist')).toContain(
      '<string>com.apple.widgetkit-extension</string>',
    );
  });
});
