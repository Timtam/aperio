const {
  withAndroidManifest,
  withDangerousMod,
  AndroidConfig,
} = require('@expo/config-plugins');
const fs = require('fs');
const path = require('path');

// Custom notification sounds on Android need a per-sound NotificationChannel
// whose sound is a content:// URI the system-UI process can read — expo-
// notifications can only resolve build-time res/raw sounds, so a runtime-
// imported file (<filesDir>/assets/sounds/<sha256>.<ext>) needs its own
// FileProvider. This plugin adds a NOT-exported FileProvider with authority
// "<applicationId>.remindersounds" + a paths xml scoping it to the sounds dir;
// the native CalFfiModule.ensureCustomSoundChannel hands the system UI a content
// URI from it (with a read grant). iOS has no equivalent — there custom sounds
// preview in-app only.

const PROVIDER_AUTHORITY = '${applicationId}.remindersounds';

const PATHS_XML = `<?xml version="1.0" encoding="utf-8"?>
<paths xmlns:android="http://schemas.android.com/apk/res/android">
    <files-path name="reminder_sounds" path="assets/sounds/" />
</paths>
`;

/** Write res/xml/reminder_sound_paths.xml into the generated Android project. */
function withReminderSoundPathsXml(config) {
  return withDangerousMod(config, [
    'android',
    async (cfg) => {
      const xmlDir = path.join(
        cfg.modRequest.platformProjectRoot,
        'app',
        'src',
        'main',
        'res',
        'xml',
      );
      fs.mkdirSync(xmlDir, { recursive: true });
      fs.writeFileSync(path.join(xmlDir, 'reminder_sound_paths.xml'), PATHS_XML);
      return cfg;
    },
  ]);
}

/** Declare the FileProvider in the AndroidManifest (idempotent). */
function withReminderSoundProviderManifest(config) {
  return withAndroidManifest(config, (cfg) => {
    const app = AndroidConfig.Manifest.getMainApplicationOrThrow(cfg.modResults);
    app.provider = app.provider ?? [];
    const exists = app.provider.some(
      (p) => p.$ && p.$['android:authorities'] === PROVIDER_AUTHORITY,
    );
    if (!exists) {
      app.provider.push({
        $: {
          'android:name': 'androidx.core.content.FileProvider',
          'android:authorities': PROVIDER_AUTHORITY,
          'android:exported': 'false',
          'android:grantUriPermissions': 'true',
        },
        'meta-data': [
          {
            $: {
              'android:name': 'android.support.FILE_PROVIDER_PATHS',
              'android:resource': '@xml/reminder_sound_paths',
            },
          },
        ],
      });
    }
    return cfg;
  });
}

module.exports = function withReminderSoundProvider(config) {
  return withReminderSoundProviderManifest(withReminderSoundPathsXml(config));
};
