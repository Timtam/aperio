const { APP_GROUP } = require('../../plugins/withAppGroup');

/**
 * The widget extension target.
 *
 * Step 1 of the widget work: this exists to prove the target is generated,
 * signed and installed. It shows a fixed line and reads nothing — the database
 * move and the real data come in step 2.
 *
 * The App Group is declared here EXPLICITLY rather than relying on the
 * plugin's mirror. `@bacons/apple-targets` copies
 * `ios.entitlements['com.apple.security.application-groups']` from app.json —
 * and ours is not there: it is added at mod time by `plugins/withAppGroup.js`,
 * which the mirror does not see. Importing the constant from that plugin keeps
 * the app and the extension on one string; two hand-typed copies that differ by
 * a character produce a container the other side cannot open, and nothing
 * errors — the widget simply finds no database.
 *
 * @type {import('@bacons/apple-targets/app.plugin').Config}
 */
module.exports = {
  type: 'widget',
  name: 'AperioWidgets',
  // Shown under the app's name in the widget gallery.
  displayName: 'Aperio',
  // `com.aperio.mobile.widget` — the leading dot appends to the app's id.
  //
  // Written down rather than derived, because this string is not only ours: an
  // extension is a SECOND app id at Apple, with its own App ID registration,
  // its own provisioning profile, and its own App Groups capability. The first
  // build with this target failed for exactly that — EAS found no credentials
  // for it and cannot mint them without an interactive login. Whatever is
  // registered in the developer account has to match this line.
  bundleIdentifier: '.widget',
  frameworks: ['SwiftUI', 'WidgetKit'],
  entitlements: {
    'com.apple.security.application-groups': [APP_GROUP],
  },
  // Interactive widgets — the tick-off in step 4 — need iOS 17. Set now so the
  // floor is not raised later, in a step where a build failure would be one
  // more thing to disentangle.
  deploymentTarget: '17.0',
};
