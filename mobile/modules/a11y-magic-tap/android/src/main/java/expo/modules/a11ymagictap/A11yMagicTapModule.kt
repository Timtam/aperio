package expo.modules.a11ymagictap

import android.content.Context
import expo.modules.kotlin.AppContext
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.viewevent.EventDispatcher
import expo.modules.kotlin.views.ExpoView

// Android counterpart of the iOS A11yMagicTapView. TalkBack has no VoiceOver-style
// two-finger double-tap "magic tap" gesture, so there is nothing to intercept
// here — this is a plain pass-through container (the screens render the native
// view on iOS only). It exists so the `A11yMagicTap` module + `onMagicTap` event
// are defined on both platforms and `requireNativeViewManager` resolves.
// `onMagicTap` is declared for parity but never fires on Android.
class A11yMagicTapView(context: Context, appContext: AppContext) :
  ExpoView(context, appContext) {
  val onMagicTap by EventDispatcher()
}

class A11yMagicTapModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("A11yMagicTap")

    View(A11yMagicTapView::class) {
      Events("onMagicTap")
    }
  }
}
