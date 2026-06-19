const { withEntitlementsPlist } = require('@expo/config-plugins');

// Aperio uses ONLY local notifications (the reminder scheduler:
// scheduleNotificationAsync + Android channels); it never registers for remote
// APNs push. iOS build #5 failed at "Run fastlane" because the generated
// entitlements requested `aps-environment` while our provisioning profile
// carries no Push Notifications capability. This plugin deletes the key from
// the generated entitlements so code-signing matches a no-push profile — build
// #6 passed with it in place.
//
// Local notifications never need this entitlement — including ones fired later
// from a background-fetch-detected change (that path needs Background Modes, a
// SEPARATE capability, not Push). Only server-driven / silent remote push needs
// aps-environment; if we ever add that, remove this plugin and enable the
// capability via `eas credentials`.
//
// Ordering note: Expo runs the LAST-registered plugin's entitlements mod FIRST
// among user mods (base providers read first / write last; user mods run in
// reverse registration order). The delete is effective regardless here because
// nothing in app.json's `plugins` re-adds aps-environment afterwards. If a
// future plugin DID add it as a user mod, this would have to run after it.
module.exports = function withoutPushEntitlement(config) {
  return withEntitlementsPlist(config, (cfg) => {
    delete cfg.modResults['aps-environment'];
    return cfg;
  });
};
