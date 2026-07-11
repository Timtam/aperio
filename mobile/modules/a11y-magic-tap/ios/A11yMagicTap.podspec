Pod::Spec.new do |s|
  s.name           = 'A11yMagicTap'
  s.version        = '1.0.0'
  s.summary        = 'VoiceOver magic-tap (two-finger double-tap) container for Aperio'
  s.description    = 'A pass-through Expo view that overrides accessibilityPerformMagicTap so a screen can run its primary action on a VoiceOver two-finger double-tap. Reacts Native onMagicTap prop is unwired on the New Architecture, so this bypasses it natively.'
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
