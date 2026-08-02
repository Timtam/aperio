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
// ORDERING — and this plugin must stay FIRST in app.json's `plugins` array.
//
// Expo runs the LAST-registered plugin's entitlements mod FIRST among user mods
// (each `withMod` calls its own action and then delegates to the previously
// registered one). So registering first means running LAST, which is the only
// position from which a delete survives.
//
// The note that used to stand here said the delete was "effective regardless
// because nothing re-adds aps-environment afterwards", with a warning that a
// future plugin adding it would have to run first. That warning was already
// describing the present: `expo-notifications` DOES add it, in
// withNotificationsIOS.js — `if (!config.modResults['aps-environment'])` — and
// it was registered EARLIER than this plugin, so it ran LATER and put the key
// straight back. The delete had quietly stopped working.
//
// It surfaced when a build finally reached Apple's profile validation: EAS's
// capability sync read the requested entitlements and switched Push
// Notifications ON at the developer portal by itself, on an app that never
// registers for remote push.
//
// Verifiable without a build, which is how this was settled:
//   cd mobile && npx expo config --type introspect | grep -c aps-environment
// Two hits with this plugin registered late, zero with it registered first.
// Run that before trusting any reordering of the plugins array.
module.exports = function withoutPushEntitlement(config) {
  return withEntitlementsPlist(config, (cfg) => {
    delete cfg.modResults['aps-environment'];
    return cfg;
  });
};
