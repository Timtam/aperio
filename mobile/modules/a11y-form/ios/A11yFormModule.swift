import ExpoModulesCore
import UIKit

// A form's content as ONE ordered VoiceOver container, the way a table is.
//
// While the software keyboard is up, VoiceOver drops every view that lies
// wholly behind the keyboard from its swipe order, and sorts the app's and the
// keyboard's elements together by their place on screen: the swipe after the
// last field above the keyboard lands on a key (decompiled UIKit accessibility
// of iOS 18.2 and 26.1; Toni, builds 72-76). A container that lists its
// children itself (`accessibilityElements`) is one entry in that sort, and its
// children are walked by index without the keyboard filter; Apple's forms are
// tables, which work this way, and VoiceOver scrolls a child out from behind
// the keyboard when it gets there.
//
// With `ordered` on, this view lists the form's elements in the order of its
// view tree, computed on every request so React Native's mounts and unmounts
// are always current: an accessibility element as it is, a view that already
// lists its own children (a text with links, a native control, a SwiftUI host)
// as one entry, anything else searched further; hidden views are left out.
// With it off, the view is a plain one. Part of the form lab of the diagnostic
// build (FormScrollView reads the switch).
class A11yFormContainerView: ExpoView {
  var ordered = false
  /// Set while the list is being built. React Native's text views ask every
  /// view above them for its accessibilityElements while they work out their
  /// own (RCTParagraphComponentView isAccessibilityCoopted), which reaches
  /// this getter again: that nested call gets the plain answer, or the walk
  /// would never end.
  private var collecting = false
  /// The list of this run-loop turn: VoiceOver asks for it again for every
  /// text it reads, and a full walk each time would cost one walk per text.
  private var cached: [Any]?

  private static let reactViewClass: AnyClass? = NSClassFromString("RCTViewComponentView")

  override var accessibilityElements: [Any]? {
    get {
      guard ordered, !collecting else { return super.accessibilityElements }
      if let cached = cached { return cached }
      collecting = true
      defer { collecting = false }
      var elements: [Any] = []
      A11yFormContainerView.collect(in: self, into: &elements)
      cached = elements
      DispatchQueue.main.async { [weak self] in
        self?.cached = nil
      }
      return elements
    }
    set {
      super.accessibilityElements = newValue
    }
  }

  /// React Native's own views are flattened; an element, a view that lists
  /// its own children (a text with its runs, a native control, a SwiftUI host)
  /// and any other native subtree with children of its own go in as one entry.
  /// A modal view stands alone, as VoiceOver would treat it.
  private static func collect(in view: UIView, into elements: inout [Any]) {
    for child in view.subviews {
      if child.isHidden || child.alpha < 0.01 || child.accessibilityElementsHidden {
        continue
      }
      if child.accessibilityViewIsModal {
        elements = [child]
        return
      }
      if child.isAccessibilityElement {
        elements.append(child)
        continue
      }
      if let own = child.accessibilityElements, !own.isEmpty {
        elements.append(child)
        continue
      }
      let isReactView = reactViewClass.map { child.isKind(of: $0) } ?? false
      if !isReactView {
        let count = child.accessibilityElementCount()
        if count != NSNotFound && count > 0 {
          elements.append(child)
          continue
        }
      }
      collect(in: child, into: &elements)
    }
  }
}

public class A11yFormModule: Module {
  public func definition() -> ModuleDefinition {
    Name("A11yForm")

    View(A11yFormContainerView.self) {
      Prop("ordered") { (view: A11yFormContainerView, ordered: Bool) in
        view.ordered = ordered
      }
    }
  }
}
