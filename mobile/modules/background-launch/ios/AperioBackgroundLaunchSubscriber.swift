import ExpoModulesCore
import Expo
import UIKit

// Starts React Native without a window when iOS launches Aperio in the
// background and no scene connects.
//
// Under the UIKit scene life cycle, which the iOS 27 SDK requires and Expo SDK
// 58 adopts, Expo starts React Native only in
// `ExpoAppSceneDelegate.scene(_:willConnectTo:options:)`. When iOS relaunches a
// terminated Aperio to run a background task (the short `BGAppRefreshTask` of
// `cal-ffi/ios/BackgroundRefresh.swift`, or expo-background-task's overnight
// `BGProcessingTask`), it connects no scene, so no JavaScript ran: the task
// service queued the task for a task manager that never registered, and the
// round — sync, reminders, Live Activities, widget — did nothing until the app
// was next opened. Expo's issue: expo/expo#52878 (open at the time of
// writing, confirmed on a real device).
//
// So: shortly after launch, if no scene has connected, start the React host
// without a view. Evaluating the bundle defines the background task, and
// expo-task-manager hands it the queued event. A scene that connects later
// starts React Native the usual way; the root view factory finds the running
// host and only adds its surface. That host started without the scene's launch
// options, and React Native sends no event for the link a connecting scene
// brings; expo-linking's app-delegate subscriber records it, and the reminder
// tap and the file import read it from there (`Linking.getLinkingURL`).
//
// Debug builds (the dev client) are left alone: its launcher picks the bundle
// and needs a window to do it.
public final class AperioBackgroundLaunchSubscriber: ExpoAppDelegateSubscriber {
  /// UIKit connects a launch's scene after `didFinishLaunching` returns, as an
  /// event of its own, tens of milliseconds later. Only well after that does
  /// "no scene" mean a launch into the background. Far below the half minute a
  /// background task gets.
  private static let sceneGrace: DispatchTimeInterval = .seconds(2)

  public func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    DispatchQueue.main.asyncAfter(deadline: .now() + Self.sceneGrace) {
      Self.startIfNoScene()
    }
    return true
  }

  private static func startIfNoScene() {
    if EXAppDefines.APP_DEBUG {
      return
    }
    guard UIApplication.shared.connectedScenes.isEmpty,
      let factory = (UIApplication.shared.delegate as? ExpoReactNativeFactoryProvider)?.reactNativeFactory,
      !AperioHasReactHost(factory)
    else {
      return
    }
    NSLog("[Aperio] background launch without a scene: starting React Native without a window")
    AperioStartReactHost(factory)
  }
}
