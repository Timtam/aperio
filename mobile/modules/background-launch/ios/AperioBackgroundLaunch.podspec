Pod::Spec.new do |s|
  s.name           = 'AperioBackgroundLaunch'
  s.version        = '1.0.0'
  s.summary        = 'Starts React Native without a window when iOS launches Aperio in the background'
  s.description    = 'Under the UIKit scene life cycle (Expo SDK 58) React Native starts only when a scene connects. A launch by iOS into the background for a background task connects none, so no JavaScript ran (expo/expo#52878). This starts the React host without a view when no scene has connected shortly after launch.'
  s.author         = 'Aperio Contributors'
  s.homepage       = 'https://github.com/Timtam/aperio'
  s.platforms      = {
    :ios => '16.4'
  }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'
  # `ExpoReactNativeFactoryProvider` and the React app-delegate headers
  # (RCTReactNativeFactory, RCTRootViewFactory) come with Expo's own pod.
  s.dependency 'Expo'
  s.dependency 'React-RCTAppDelegate'

  s.source_files = ['*.swift', 'AperioReactHost.h', 'AperioReactHost.mm']

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }
end
