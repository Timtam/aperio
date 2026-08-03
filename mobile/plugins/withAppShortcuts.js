const { withDangerousMod, withXcodeProject, IOSConfig } = require('@expo/config-plugins');
const fs = require('fs');
const path = require('path');

// Puts Aperio's Siri shortcuts into the generated iOS APP target.
//
// Not a preference — a requirement. An `AppShortcutsProvider` is only
// discovered in the main app target, and the framework escape hatch
// (`AppIntentsPackage`) covers frameworks, not the static libraries Expo
// modules compile to. So the one place this Swift can live is the app target,
// which in a managed project only exists on the build machine.
//
// Two mods, because they answer two different questions: the files have to BE
// there (a dangerous mod, writing into the generated tree) and Xcode has to
// BUILD them (a project mod, adding them to the target's phases). Doing only
// the first produces a build that succeeds and a Siri that has never heard of
// us.
//
// The `.lproj` folders are the same story one level down. Siri matches spoken
// input against the phrase set for the language it is set to, and reads that
// set out of the app bundle — so a German phrase is a RESOURCE that has to be
// copied and registered, not a string in the Swift. Without this, a German Siri
// only ever sees the English phrases and hands "erstelle einen Termin" to
// Apple's own Calendar.

const SOURCE = 'AperioShortcuts.swift';

/** Languages that get an `.lproj` in the app bundle. English is the
 *  development language and comes from the Swift literals themselves, so it is
 *  deliberately absent — an `en.lproj` would only be a second place for the
 *  same words to drift apart in. */
const LOCALIZATIONS = ['de'];
/** Both are read by name. `AppShortcuts.strings` is the ONLY file consulted for
 *  spoken phrases; everything else about an intent — titles, descriptions,
 *  parameter labels, the questions Siri asks — comes from `Localizable.strings`.
 *  Putting a phrase in the second one compiles and then does nothing. */
const STRINGS = ['AppShortcuts.strings', 'Localizable.strings'];

/** Everything this plugin installs, as paths relative to the app target's
 *  folder. One list, so the copying mod and the registering mod cannot drift. */
function localizedStrings() {
  return LOCALIZATIONS.flatMap((lang) => STRINGS.map((file) => `${lang}.lproj/${file}`));
}

/** Copy `mobile/ios-app/**` into the app target's folder.
 *
 *  Kept as real files in the repo rather than strings in here: the Swift
 *  deserves to be read, diffed and edited as Swift, and the translations
 *  deserve to sit where a translator would look for them. */
function withCopiedSources(config) {
  return withDangerousMod(config, [
    'ios',
    async (cfg) => {
      const from = path.join(cfg.modRequest.projectRoot, 'ios-app');
      const into = path.join(cfg.modRequest.platformProjectRoot, cfg.modRequest.projectName);
      for (const relative of [SOURCE, ...localizedStrings()]) {
        const destination = path.join(into, relative);
        fs.mkdirSync(path.dirname(destination), { recursive: true });
        fs.copyFileSync(path.join(from, relative), destination);
      }
      return cfg;
    },
  ]);
}

/** Add them to the app target: the Swift to Compile Sources, the strings to
 *  Copy Bundle Resources.
 *
 *  The `.lproj` in a resource's PATH is what makes it a localization — Xcode
 *  reads the language off the folder, and no variant group is involved. That is
 *  the same shape Expo's own `locales` support uses for `InfoPlist.strings`.
 *
 *  All of it goes into the app target's own group, whose entries carry paths
 *  relative to the project root; that is proven by the Swift file, which has
 *  been compiling from exactly this arrangement. One consequence worth knowing
 *  before adding a second language: a group holds at most one child per FILE
 *  NAME, so `de.lproj/AppShortcuts.strings` and a future
 *  `fr.lproj/AppShortcuts.strings` would collide here and the second would be
 *  silently skipped. At that point each `.lproj` needs its own group.
 *
 *  The other thing worth knowing before changing app.json: Expo's own `locales`
 *  option writes ITS `Localizable.strings` to `Supporting/<lang>.lproj/`. Two
 *  different source paths, one destination in the bundle — Xcode fails that with
 *  "multiple commands produce", naming neither cause. `locales` is not set
 *  today; if it ever is, these translations belong in it rather than here. */
function withSourcesBuilt(config) {
  return withXcodeProject(config, (cfg) => {
    const project = cfg.modResults;
    const group = cfg.modRequest.projectName;
    // A prebuild that ran twice would otherwise list a file twice — for the
    // Swift that is a duplicate-symbol error naming nothing useful.
    const isKnown = (relative) =>
      Object.values(project.pbxFileReferenceSection()).some(
        (entry) => typeof entry === 'object' && entry.path && entry.path.includes(relative),
      );

    if (!isKnown(SOURCE)) {
      IOSConfig.XcodeUtils.addBuildSourceFileToGroup({
        filepath: `${group}/${SOURCE}`,
        groupName: group,
        project,
      });
    }
    for (const relative of localizedStrings()) {
      if (isKnown(relative)) continue;
      IOSConfig.XcodeUtils.addResourceFileToGroup({
        filepath: `${group}/${relative}`,
        groupName: group,
        project,
        isBuildFile: true,
      });
    }
    return cfg;
  });
}

module.exports = function withAppShortcuts(config) {
  return withSourcesBuilt(withCopiedSources(config));
};
