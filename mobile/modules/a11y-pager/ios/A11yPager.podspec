Pod::Spec.new do |s|
  s.name           = 'A11yPager'
  s.version        = '1.0.0'
  s.summary        = 'VoiceOver three-finger-swipe pager container for Aperio'
  s.description    = 'A pass-through Expo view that overrides accessibilityScroll: so the calendar views page on a VoiceOver three-finger swipe without the default "page X of N" announcement.'
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
