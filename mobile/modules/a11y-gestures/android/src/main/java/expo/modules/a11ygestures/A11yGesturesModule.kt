package expo.modules.a11ygestures

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

// Android counterpart of the iOS A11yGesturesModule. TalkBack has no
// VoiceOver-style two-finger double-tap "magic tap" gesture, and the
// horizontal three-finger scroll-to-page concept is iOS-only, so this module
// declares the same `magicTap` / `page` events for parity but NEVER emits them.
// It exists so `requireNativeModule('A11yGestures')` resolves on both platforms;
// the JS gesture host simply never receives an event on Android.
class A11yGesturesModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("A11yGestures")

    Events("magicTap", "page")
  }
}
