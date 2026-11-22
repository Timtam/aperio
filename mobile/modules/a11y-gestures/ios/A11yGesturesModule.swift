import ExpoModulesCore
import UIKit
import ObjectiveC.runtime

private let magicTapNotification = Notification.Name("AperioMagicTap")
private let scrollNotification = Notification.Name("AperioScroll")
private let labLogNotification = Notification.Name("AperioFormLabLog")

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
// UIWindow via `class_replaceMethod` is reliable. Each override posts a
// NSNotification that the module forwards to JS.
private enum WindowGestureInstaller {
  static var installed = false
  /// The `accessibilityScroll:` a UIWindow answered with before ours (its own
  /// or an inherited one), so the form lab can hand directions back to it.
  static var previousScroll: IMP?
  /// Whether UIWindow defined `accessibilityScroll:` itself before ours, for
  /// the form lab's log.
  static var windowHadOwnScroll = false

  typealias ScrollFunction = @convention(c) (AnyObject, Selector, UIAccessibilityScrollDirection) -> Bool

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
      #selector(UIWindow.accessibilityPerformMagicTap),
      imp_implementationWithBlock(magicBlock),
      "B@:"
    )

    let scrollSelector = #selector(UIWindow.accessibilityScroll(_:))
    let windowScroll = class_getInstanceMethod(UIWindow.self, scrollSelector).map(method_getImplementation)
    let viewScroll = class_getInstanceMethod(UIView.self, scrollSelector).map(method_getImplementation)
    previousScroll = windowScroll
    windowHadOwnScroll = windowScroll != nil && windowScroll != viewScroll

    // -[UIWindow accessibilityScroll:] -> BOOL. Horizontal → page; vertical is
    // left to the default (return false) so lists still scroll. The form lab
    // (FormLab.windowGestures) can narrow this to left/right or hand every
    // direction back to the previous implementation.
    let scrollBlock: @convention(block) (AnyObject, UIAccessibilityScrollDirection) -> Bool = {
      window, direction in
      let mode = FormLab.windowGestures
      let handled: Bool
      switch (mode, direction) {
      case ("off", _):
        handled = WindowGestureInstaller.forwardScroll(window, direction)
      case ("normal", .left), ("normal", .next), ("narrow", .left):
        NotificationCenter.default.post(
          name: scrollNotification, object: nil, userInfo: ["direction": "next"])
        handled = true
      case ("normal", .right), ("normal", .previous), ("narrow", .right):
        NotificationCenter.default.post(
          name: scrollNotification, object: nil, userInfo: ["direction": "prev"])
        handled = true
      case ("normal", _):
        handled = false
      default:
        handled = WindowGestureInstaller.forwardScroll(window, direction)
      }
      FormLab.log("window scroll dir=\(direction.rawValue) mode=\(mode) handled=\(handled)")
      return handled
    }
    class_replaceMethod(
      UIWindow.self,
      scrollSelector,
      imp_implementationWithBlock(scrollBlock),
      "B@:q"
    )
  }

  static func forwardScroll(_ window: AnyObject, _ direction: UIAccessibilityScrollDirection) -> Bool {
    guard let previous = previousScroll else { return false }
    let call = unsafeBitCast(previous, to: ScrollFunction.self)
    return call(window, #selector(UIWindow.accessibilityScroll(_:)), direction)
  }
}

// MARK: - VoiceOver form lab (temporary, a diagnostic build)
//
// With the software keyboard up, VoiceOver drops every view that lies wholly
// behind the keyboard from its swipe order and sorts the app's and the
// keyboard's elements together by position, so the swipe after the last field
// above the keyboard lands on a key (Toni, builds 72-76; decompiled UIKit
// accessibility of iOS 18.2 and 26.1, research wf_4b33d7c5-1ce). This lab lets
// the tester switch candidate fixes at run time and compare them in one
// build; the winner is built in properly and the lab goes again:
// - ordered container: the form lists its fields itself, as a table does
//   (modules/a11y-form, read from JS);
// - focusFollows: when VoiceOver focus lands in a form while the keyboard is
//   up, the form scrolls so the next field is above the keyboard;
// - resignOnLeave: when VoiceOver focus leaves the text field, the keyboard
//   closes;
// - windowGestures: the window's accessibilityScroll as before, narrowed to
//   left/right, or off (handed back to UIKit);
// - log: what happened, without any label or value, into Aperio's log.
// All flags live in memory only and start off, so a build without the lab's
// settings section behaves exactly as before.
enum FormLab {
  static var windowGestures = "normal"
  static var focusFollows = false
  static var resignOnLeave = false
  static var logging = false
  static var observing = false
  private static var observers: [NSObjectProtocol] = []
  /// The keyboard's end frame in screen coordinates; nil while it is hidden.
  static var keyboardFrame: CGRect?

  private static var active: Bool { focusFollows || resignOnLeave || logging }

  static func apply(_ flags: [String: Any]) {
    if let mode = flags["windowGestures"] as? String, ["normal", "narrow", "off"].contains(mode) {
      windowGestures = mode
    }
    focusFollows = (flags["focusFollows"] as? Bool) ?? false
    resignOnLeave = (flags["resignOnLeave"] as? Bool) ?? false
    logging = (flags["log"] as? Bool) ?? false
    if active { observeIfNeeded() } else { stopObserving() }
    log(
      "flags windowGestures=\(windowGestures) focusFollows=\(focusFollows) "
        + "resignOnLeave=\(resignOnLeave) windowHadOwnScroll=\(WindowGestureInstaller.windowHadOwnScroll)"
    )
  }

  static func log(_ line: @autoclosure () -> String) {
    guard logging else { return }
    NotificationCenter.default.post(name: labLogNotification, object: nil, userInfo: ["line": line()])
  }

  private static func stopObserving() {
    observers.forEach { NotificationCenter.default.removeObserver($0) }
    observers = []
    observing = false
    keyboardFrame = nil
  }

  private static func observeIfNeeded() {
    guard !observing else { return }
    observing = true
    let center = NotificationCenter.default
    observers.append(
      center.addObserver(
        forName: UIResponder.keyboardWillChangeFrameNotification, object: nil, queue: .main
      ) { note in
        guard let frame = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue
        else { return }
        // A keyboard that slides away ends below the screen with its full
        // height: on screen only while its top is above the screen's bottom.
        let screenBottom = (note.object as? UIScreen)?.bounds.maxY ?? .greatestFiniteMagnitude
        FormLab.keyboardFrame = frame.height > 0 && frame.minY < screenBottom - 1 ? frame : nil
      })
    for name in [UIResponder.keyboardWillHideNotification, UIResponder.keyboardDidHideNotification] {
      observers.append(
        center.addObserver(forName: name, object: nil, queue: .main) { _ in
          FormLab.keyboardFrame = nil
        })
    }
    for name in [UIResponder.keyboardDidShowNotification, UIResponder.keyboardDidChangeFrameNotification] {
      observers.append(
        center.addObserver(forName: name, object: nil, queue: .main) { _ in
          FormLab.afterKeyboardMoved()
        })
    }
    observers.append(
      center.addObserver(
        forName: UIAccessibility.elementFocusedNotification, object: nil, queue: .main
      ) { note in
        guard UIAccessibility.isVoiceOverRunning else { return }
        FormLab.focused(note.userInfo?[UIAccessibility.focusedElementUserInfoKey])
      })
  }

  private static func afterKeyboardMoved() {
    guard active, UIAccessibility.isVoiceOverRunning, keyboardFrame != nil else { return }
    let element = UIAccessibility.focusedElement(using: .notificationVoiceOver)
    log("keyboard shown, focus \(shortName(element))")
    if focusFollows { keepNextAboveKeyboard(element) }
  }

  private static func focused(_ element: Any?) {
    guard active, keyboardFrame != nil else { return }
    let view = viewOf(element)
    if logging, let view = view, let window = view.window {
      let frame = view.convert(view.bounds, to: window)
      let keyboardTop = keyboardFrame.map { window.convert($0, from: nil as UIView?).minY } ?? -1
      let scroll = enclosingScroll(view)
      log(
        "focus \(shortName(element)) view=\(shortName(view)) minY=\(Int(frame.minY)) maxY=\(Int(frame.maxY)) "
          + "keyboardTop=\(Int(keyboardTop)) offset=\(Int(scroll?.contentOffset.y ?? -1)) "
          + "insetBottom=\(Int(scroll?.adjustedContentInset.bottom ?? -1))"
      )
    }
    if resignOnLeave { resignIfLeaving(view) }
    if focusFollows { keepNextAboveKeyboard(element) }
  }

  /// Scroll the form so the focused element ends a little below the middle of
  /// the part above the keyboard: the next field is then on screen above it,
  /// and VoiceOver swipes onto it instead of into the keyboard.
  private static func keepNextAboveKeyboard(_ element: Any?) {
    guard let keyboard = keyboardFrame, let view = viewOf(element),
      let scroll = enclosingScroll(view), scroll.window != nil
    else { return }
    let rect = scroll.convert(view.bounds, from: view)
    // The keyboard's frame is in screen coordinates; an app window covers the
    // screen, so they are the window's too (a page sheet is a view in it).
    let keyboardInScroll = scroll.convert(keyboard, from: nil as UIView?)
    let visibleTop = scroll.bounds.minY
    let visibleBottom = min(scroll.bounds.maxY, keyboardInScroll.minY)
    let room = visibleBottom - visibleTop
    guard room > 60, rect.maxY > visibleTop + 0.5 * room else { return }
    let lowest = -scroll.adjustedContentInset.top
    let highest = max(
      lowest, scroll.contentSize.height - scroll.bounds.height + scroll.adjustedContentInset.bottom)
    let target = min(max(rect.maxY - 0.45 * room, lowest), highest)
    guard target > scroll.contentOffset.y + 1 else { return }
    scroll.setContentOffset(CGPoint(x: scroll.contentOffset.x, y: target), animated: false)
    log("focusFollows scrolled to \(Int(target))")
  }

  private static func resignIfLeaving(_ view: UIView?) {
    guard let view = view, let responder = FirstResponder.current() as? UIView,
      responder is UITextInput, responder.window === view.window
    else { return }
    if view === responder || view.isDescendant(of: responder) || responder.isDescendant(of: view) {
      return
    }
    responder.resignFirstResponder()
    log("resignOnLeave closed the keyboard")
  }

  /// The view behind an accessibility element: the element itself, or the
  /// first view up its container chain (a text paragraph's element, a SwiftUI
  /// node).
  private static func viewOf(_ element: Any?) -> UIView? {
    var current: Any? = element
    var hops = 0
    while let object = current, hops < 12 {
      if let view = object as? UIView { return view }
      guard let node = object as? NSObject,
        node.responds(to: NSSelectorFromString("accessibilityContainer"))
      else { return nil }
      current = node.value(forKey: "accessibilityContainer")
      hops += 1
    }
    return nil
  }

  /// The form's scroll view: the first scroll view above the element that is
  /// not a text view (a multiline input is a UITextView, itself a scroll view).
  private static func enclosingScroll(_ view: UIView) -> UIScrollView? {
    var current = view.superview
    while let candidate = current {
      if let scroll = candidate as? UIScrollView, !(scroll is UITextView), scroll.window === view.window {
        return scroll
      }
      current = candidate.superview
    }
    return nil
  }

  /// A type name short enough for the log's redaction (it hides runs of 32 or
  /// more word characters).
  private static func shortName(_ object: Any?) -> String {
    guard let object = object else { return "none" }
    let name = String(describing: type(of: object)).split(separator: ".").last.map(String.init) ?? "?"
    return String(name.prefix(28))
  }
}

/// The current first responder, found by sending an action to nil (UIKit
/// delivers it to the first responder).
private enum FirstResponder {
  static weak var found: UIResponder?

  static func current() -> UIResponder? {
    found = nil
    UIApplication.shared.sendAction(
      #selector(UIResponder.aperioCaptureFirstResponder), to: nil, from: nil, for: nil)
    return found
  }
}

extension UIResponder {
  @objc fileprivate func aperioCaptureFirstResponder() {
    FirstResponder.found = self
  }
}

// Forwards the window-level VoiceOver gestures to JS as module events. JS
// (src/a11y/gestureHost.ts) routes them to the currently focused screen's action
// / pager. The overrides communicate via NSNotification so the install site and
// the emitter stay decoupled.
public class A11yGesturesModule: Module {
  public func definition() -> ModuleDefinition {
    Name("A11yGestures")

    Events("magicTap", "page", "labLog")

    // The form lab's switches (see FormLab), pushed by src/a11y/formLab.ts.
    Function("setLabFlags") { (flags: [String: Any]) in
      DispatchQueue.main.async {
        FormLab.apply(flags)
      }
    }

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
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(self.handleLabLog(_:)),
        name: labLogNotification,
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

  @objc
  func handleLabLog(_ notification: Notification) {
    let line = (notification.userInfo?["line"] as? String) ?? ""
    self.sendEvent("labLog", ["line": line])
  }
}
