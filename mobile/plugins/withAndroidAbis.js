const { withGradleProperties } = require('@expo/config-plugins');

// Package exactly the ABIs we actually build the Rust core for.
//
// Expo's prebuild writes `reactNativeArchitectures=armeabi-v7a,arm64-v8a,x86,
// x86_64` into android/gradle.properties, but the Android CI workflow
// (.github/workflows/mobile-android-play.yml) runs
// `cargo ndk -t arm64-v8a -t x86_64`. Play splits an app bundle per ABI, so the
// armeabi-v7a and x86 splits would ship every React Native library EXCEPT
// libcal_ffi.so — and a 32-bit device would install one of them and die with
// UnsatisfiedLinkError the moment anything touched the core. Nothing in the
// build fails; the crash only appears on a device nobody tests on.
//
// So the list is narrowed here rather than widened in CI. 32-bit ARM phones are
// long past relevance for a new app (minSdk 24 notwithstanding), and building
// the Rust core for two more targets would double CI time to serve them.
// x86_64 stays because it is what an emulator runs on.
//
// If you ever add an ABI here, add the matching `-t` to the cargo-ndk step in
// the workflow in the same commit. The two lists are one decision.
const ABIS = 'arm64-v8a,x86_64';

module.exports = function withAndroidAbis(config) {
  return withGradleProperties(config, (cfg) => {
    const existing = cfg.modResults.find(
      (item) => item.type === 'property' && item.key === 'reactNativeArchitectures',
    );
    if (existing) {
      existing.value = ABIS;
    } else {
      cfg.modResults.push({
        type: 'property',
        key: 'reactNativeArchitectures',
        value: ABIS,
      });
    }
    return cfg;
  });
};
