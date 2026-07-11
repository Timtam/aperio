Pod::Spec.new do |s|
  s.name           = 'A11yGestures'
  s.version        = '1.0.0'
  s.summary        = 'App-wide VoiceOver gesture host for Aperio (magic tap + 3-finger scroll)'
  s.description    = 'A UIWindow category that catches VoiceOver magic tap + horizontal three-finger scroll at the window (the last responder before the app delegate) and forwards them to JS, so the gestures fire even when focus is on a UIAccessibilityElement that is not a real UIView responder (a heading / chrome control) and never bubbles to a mid-tree native view.'
  s.author         = 'Aperio Contributors'
  s.homepage       = 'https://github.com/Timtam/aperio'
  s.platforms      = {
    :ios => '16.4'
  }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = '*.swift'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }
end
