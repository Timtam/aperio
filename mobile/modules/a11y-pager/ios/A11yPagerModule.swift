import ExpoModulesCore
import UIKit

// A pass-through container that intercepts VoiceOver's three-finger swipe and
// forwards its horizontal direction to JS, so the calendar views can page
// WITHOUT the "page X of N" announcement a real UIScrollView emits and without
// throwing the reader's focus onto a hidden spacer page (the problems the old
// three-page ScrollView pager had under VoiceOver).
//
// Mechanism: UIKit sends `accessibilityScroll(_:)` to the accessibility element
// under VoiceOver focus on a three-finger swipe, bubbling UP through the
// superview chain until one returns `true`. This view sits ABOVE the calendar's
// day list, so a HORIZONTAL swipe — which the inner vertical list can't consume,
// so it bubbles here — lands on us: we emit `onPage` (JS shifts the period and
// announces the new range) and return `true` to mark the scroll handled. We
// post an EMPTY `.pageScrolled` first so VoiceOver doesn't speak its own default
// scroll status before the JS period announcement. VERTICAL swipes fall through
// (default `false`), leaving the inner list to scroll normally.
class A11yPagerView: ExpoView {
  let onPage = EventDispatcher()

  override func accessibilityScroll(_ direction: UIAccessibilityScrollDirection) -> Bool {
    switch direction {
    case .left, .next:
      // Swipe left advances to the next period (reading-order forward). If this
      // maps backwards on device, swap next/prev here.
      page("next")
      return true
    case .right, .previous:
      page("prev")
      return true
    default:
      // Up / down: leave it to the inner day list to scroll vertically.
      return false
    }
  }

  private func page(_ direction: String) {
    // Suppress VoiceOver's own post-scroll status ("page X of N") — the screen
    // announces the new period itself once the anchor changes.
    UIAccessibility.post(notification: .pageScrolled, argument: "")
    onPage(["direction": direction])
  }
}

public class A11yPagerModule: Module {
  public func definition() -> ModuleDefinition {
    Name("A11yPager")

    View(A11yPagerView.self) {
      Events("onPage")
    }
  }
}
