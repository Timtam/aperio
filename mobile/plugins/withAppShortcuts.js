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
// Two mods, because they answer two different questions: the file has to BE
// there (a dangerous mod, writing into the generated tree) and Xcode has to
// COMPILE it (a project mod, adding it to the target's sources phase). Doing
// only the first produces a build that succeeds and a Siri that has never
// heard of us.

const SOURCE = 'AperioShortcuts.swift';

/** Copy `mobile/ios-app/AperioShortcuts.swift` into the app target's folder.
 *
 *  Kept as a real file in the repo rather than a string in here: it is Swift,
 *  it deserves to be read, diffed and edited as Swift. */
function withCopiedSource(config) {
  return withDangerousMod(config, [
    'ios',
    async (cfg) => {
      const from = path.join(cfg.modRequest.projectRoot, 'ios-app', SOURCE);
      const into = path.join(
        cfg.modRequest.platformProjectRoot,
        cfg.modRequest.projectName,
        SOURCE,
      );
      fs.mkdirSync(path.dirname(into), { recursive: true });
      fs.copyFileSync(from, into);
      return cfg;
    },
  ]);
}

/** Add it to the app target's compile sources. */
function withSourceCompiled(config) {
  return withXcodeProject(config, (cfg) => {
    const project = cfg.modResults;
    const group = cfg.modRequest.projectName;
    // A prebuild that ran twice would otherwise list the file twice and fail
    // with a duplicate-symbol error rather than anything that names the cause.
    const already = Object.values(project.pbxFileReferenceSection()).some(
      (entry) => typeof entry === 'object' && entry.path && entry.path.includes(SOURCE),
    );
    if (!already) {
      IOSConfig.XcodeUtils.addBuildSourceFileToGroup({
        filepath: `${group}/${SOURCE}`,
        groupName: group,
        project,
      });
    }
    return cfg;
  });
}

module.exports = function withAppShortcuts(config) {
  return withSourceCompiled(withCopiedSource(config));
};
