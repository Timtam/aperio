const { withEntitlementsPlist } = require('@expo/config-plugins');

// expo-notifications injects the `aps-environment` (Push Notifications)
// entitlement during prebuild. Aperio uses ONLY local notifications (the
// reminder scheduler: scheduleNotificationAsync + Android channels); it never
// registers for remote APNs push. The unused entitlement forces the iOS
// provisioning profile to carry the Push Notifications capability — ours
// doesn't, so the build's "Run fastlane" step fails. Strip it so the build
// matches a no-push profile.
//
// Local notifications never need this entitlement — including ones fired later
// from a background-fetch-detected change (that path needs Background Modes, a
// SEPARATE capability, not Push). Only server-driven / silent remote push needs
// aps-environment. If we ever add that, remove this plugin and enable the
// capability via `eas credentials`.
//
// Registered LAST in app.json's `plugins` so this entitlements mod runs after
// the one that adds the key; deleting an absent key is a harmless no-op.
module.exports = function withoutPushEntitlement(config) {
  return withEntitlementsPlist(config, (cfg) => {
    delete cfg.modResults['aps-environment'];
    return cfg;
  });
};
