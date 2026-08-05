Pod::Spec.new do |s|
  s.name           = 'CalFfi'
  s.version        = '1.0.0'
  s.summary        = 'Aperio cal-core via UniFFI (iOS)'
  s.description    = 'Exposes cal-core domain logic to the RN app through the UniFFI Swift bindings, backed by an XCFramework built by the cal-ffi-ios GitHub Actions workflow.'
  s.author         = 'Aperio Contributors'
  s.homepage       = 'https://github.com/Timtam/aperio'
  s.platforms      = {
    :ios => '16.4'
  }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  # Rust static libs (device + simulator) + the UniFFI C header & modulemap.
  s.vendored_frameworks = 'CalFfi.xcframework'

  # Only our own sources (the Expo module + the UniFFI Swift bindings).
  # Scoped to top-level *.swift so it does NOT recurse into the XCFramework's
  # headers (which would otherwise be picked up and break the build) — and the
  # one Objective-C pair is named EXPLICITLY for the same reason: a blanket
  # '*.{h,m}' would sweep up `cal_ffiFFI.h`, which arrives with the vendored
  # framework and must not be compiled a second time.
  s.source_files = ['*.swift', 'AperioTaskServiceHelper.h', 'AperioTaskServiceHelper.m']

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }
end
