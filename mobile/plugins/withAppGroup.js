const { withEntitlementsPlist } = require('@expo/config-plugins');

/** The shared container the app and, later, the widget extension both open.
 *
 *  Apple requires the `group.` prefix. Kept here rather than in app.json so the
 *  extension's own entitlement can import the same constant instead of
 *  repeating a string that must match exactly — a typo in one of two places
 *  produces a container the other side cannot see, and nothing errors: the
 *  widget simply finds no database.
 */
const APP_GROUP = 'group.com.aperio.mobile';

// §? Widgets — the shared container.
//
// A widget runs in its own process and cannot reach the app's sandbox, so the
// database has to live somewhere both can open. That place is an App Group
// container, and getting into one starts with this entitlement.
//
// This is STEP 0 of the widget work, deliberately alone: no widget target, no
// database move, nothing that depends on the answer. The question it asks is
// whether the capability signs at all against our provisioning profile — and
// that question has bitten this project before. Build #5 failed at fastlane
// because the generated entitlements asked for `aps-environment` that the
// profile did not carry, which is why `withoutPushEntitlement.js` exists next
// door. App Groups is the same class of problem, and the iOS loop is one EAS
// build long, so it gets its own build rather than being discovered three
// features deep.
//
// What has to exist on Apple's side: an App Group identifier
// `group.com.aperio.mobile`, and the App Groups capability enabled on the app
// id `com.aperio.mobile` with that group assigned. EAS syncs capabilities from
// the entitlements file on build, so it may do this itself; if the build stops
// at code signing, it did not, and the group has to be created in the developer
// portal by hand.
module.exports = function withAppGroup(config) {
  return withEntitlementsPlist(config, (cfg) => {
    const key = 'com.apple.security.application-groups';
    const groups = new Set(cfg.modResults[key] ?? []);
    groups.add(APP_GROUP);
    // Sorted, so a regenerated project produces a stable entitlements file
    // rather than a diff whose order depends on plugin evaluation.
    cfg.modResults[key] = [...groups].sort();
    return cfg;
  });
};

module.exports.APP_GROUP = APP_GROUP;
