package expo.modules.a11ypager

import android.content.Context
import expo.modules.kotlin.AppContext
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.viewevent.EventDispatcher
import expo.modules.kotlin.views.ExpoView

// Android counterpart of the iOS A11yPagerView. TalkBack has no VoiceOver-style
// three-finger scroll-to-page gesture and does not emit an iOS-like
// "page X of N" announcement, so there is nothing to intercept here — this is a
// plain pass-through container (the calendar uses it on iOS only; on Android the
// toolbar buttons drive paging). It exists so the `A11yPager` module + `onPage`
// event are defined on both platforms and `requireNativeViewManager` resolves.
// `onPage` is declared for parity but never fires on Android.
class A11yPagerView(context: Context, appContext: AppContext) :
  ExpoView(context, appContext) {
  val onPage by EventDispatcher()
}

class A11yPagerModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("A11yPager")

    View(A11yPagerView::class) {
      Events("onPage")
    }
  }
}
