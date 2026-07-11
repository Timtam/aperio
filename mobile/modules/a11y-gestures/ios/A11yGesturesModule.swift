import ExpoModulesCore
import UIKit
import ObjectiveC.runtime

private let magicTapNotification = Notification.Name("AperioMagicTap")
private let scrollNotification = Notification.Name("AperioScroll")

// Installs the window-level VoiceOver overrides ONCE.
//
// WHY THE WINDOW: iOS sends `accessibilityPerformMagicTap` to the focused element
// and, if unhandled, walks UP the responder chain to the app delegate (Apple's
// documented behaviour). The window is the last object in that chain before the
// app delegate, so a magic tap from a focused `UIAccessibilityElement` — a
// heading / chrome control that is NOT a real UIView responder and therefore
// never bubbles the gesture to a mid-tree in-content native view (A11yMagicTapView
// / A11yPagerView) — reaches here. When an in-content view DID handle it, it
// returned YES and the chain stopped, so the window never sees it (no double
// handling). `accessibilityScroll:` is handled here too as a best-effort chrome
// fallback for the three-finger swipe.
//
// WHY THE RUNTIME (not an ObjC category): a category's object file can be
// dead-stripped from a static framework when the app isn't linked with `-ObjC`,
// which would silently disable it. This Swift code is referenced by the
// registered Expo module and is guaranteed to run, so adding the methods to
// UIWindow via `class_replaceMethod` (the selectors aren't defined on UIWindow,
// so it adds rather than replaces) is reliable. Each override posts a
// NSNotification that the module forwards to JS.
private enum WindowGestureInstaller {
  static var installed = false

  static func installIfNeeded() {
    guard !installed else { return }
    installed = true

    // -[UIWindow accessibilityPerformMagicTap] -> BOOL
    let magicBlock: @convention(block) (AnyObject) -> Bool = { _ in
      NotificationCenter.default.post(name: magicTapNotification, object: nil)
      return true
    }
    class_replaceMethod(
      UIWindow.self,
      Selector(("accessibilityPerformMagicTap")),
      imp_implementationWithBlock(magicBlock),
      "B@:"
    )

    // -[UIWindow accessibilityScroll:] -> BOOL. Horizontal → page; vertical is
    // left to the default (return false) so lists still scroll.
    let scrollBlock: @convention(block) (AnyObject, UIAccessibilityScrollDirection) -> Bool = { _, direction in
      switch direction {
      case .left, .next:
        NotificationCenter.default.post(
          name: scrollNotification, object: nil, userInfo: ["direction": "next"])
        return true
      case .right, .previous:
        NotificationCenter.default.post(
          name: scrollNotification, object: nil, userInfo: ["direction": "prev"])
        return true
      default:
        return false
      }
    }
    class_replaceMethod(
      UIWindow.self,
      Selector(("accessibilityScroll:")),
      imp_implementationWithBlock(scrollBlock),
      "B@:q"
    )
  }
}

// Forwards the window-level VoiceOver gestures to JS as module events. JS
// (src/a11y/gestureHost.ts) routes them to the currently focused screen's action
// / pager. The overrides communicate via NSNotification so the install site and
// the emitter stay decoupled.
public class A11yGesturesModule: Module {
  public func definition() -> ModuleDefinition {
    Name("A11yGestures")

    Events("magicTap", "page")

    OnStartObserving {
      WindowGestureInstaller.installIfNeeded()
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(self.handleMagicTap),
        name: magicTapNotification,
        object: nil
      )
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(self.handleScroll(_:)),
        name: scrollNotification,
        object: nil
      )
    }

    OnStopObserving {
      // swiftlint:disable:next notification_center_detachment
      NotificationCenter.default.removeObserver(self)
    }
  }

  @objc
  func handleMagicTap() {
    self.sendEvent("magicTap")
  }

  @objc
  func handleScroll(_ notification: Notification) {
    let direction = (notification.userInfo?["direction"] as? String) ?? "next"
    self.sendEvent("page", ["direction": direction])
  }
}
