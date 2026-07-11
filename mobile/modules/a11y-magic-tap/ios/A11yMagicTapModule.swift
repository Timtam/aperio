import ExpoModulesCore
import UIKit

// A pass-through container that catches VoiceOver's MAGIC TAP (a two-finger
// double-tap) and forwards it to JS, so a screen can perform its single most
// relevant action — creating the screen's primary item (a new event / task /
// contact) — regardless of where VoiceOver focus currently sits.
//
// WHY NATIVE: React Native's `onMagicTap` prop is dead on the New Architecture
// (Fabric). Fabric's RCTViewComponentView only fires the handler when its raw
// prop `onAccessibilityMagicTap` is set, but the JS layer still sends the prop
// under the legacy name `onMagicTap` with no alias, so `onAccessibilityMagicTap`
// stays false and `accessibilityPerformMagicTap` always returns NO. This is the
// same class of gap that forced the a11y-pager to intercept `accessibilityScroll:`
// natively.
//
// MECHANISM: UIKit sends `accessibilityPerformMagicTap` to the element with
// VoiceOver focus and, if it returns NO, walks UP the responder chain until an
// object handles it (ending at the app delegate). This view wraps the whole
// screen's content, so it sits in that chain above every focusable element on
// the screen — the magic tap bubbles up to us, we emit `onMagicTap` and return
// `true` to mark it handled. Android (TalkBack has no such gesture) never
// instantiates this view; the module exists only so `requireNativeViewManager`
// resolves on both platforms.
class A11yMagicTapView: ExpoView {
  let onMagicTap = EventDispatcher()

  override func accessibilityPerformMagicTap() -> Bool {
    // Concrete (non-empty) payload so the dictionary literal's type infers
    // cleanly, mirroring the a11y-pager's `onPage(["direction": …])`. JS ignores
    // the payload — the event firing is the whole signal.
    onMagicTap(["handled": true])
    return true
  }
}

public class A11yMagicTapModule: Module {
  public func definition() -> ModuleDefinition {
    Name("A11yMagicTap")

    View(A11yMagicTapView.self) {
      Events("onMagicTap")
    }
  }
}
