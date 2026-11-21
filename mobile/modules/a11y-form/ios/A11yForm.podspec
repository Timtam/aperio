Pod::Spec.new do |s|
  s.name           = 'A11yForm'
  s.version        = '1.0.0'
  s.summary        = 'A form container that lists its fields to VoiceOver in order'
  s.description    = 'An Expo view whose accessibilityElements are the form fields in document order, so VoiceOver walks them like a table walks its rows, including the ones behind the keyboard.'
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
