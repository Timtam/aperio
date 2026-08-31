import { Platform } from 'react-native';

import { A11yGestures } from '../../modules/a11y-gestures';
import { navigationRef } from '../navigation/navigationRef';
import { isAppLockCoverVisible } from '../state/appLock';

// Routes the app-wide, window-level VoiceOver gestures (magic tap + horizontal
// three-finger scroll, caught natively — see modules/a11y-gestures) to the
// CURRENTLY ACTIVE screen.
//
// Each screen registers its handler under ITS ROUTE NAME on mount (MagicTapView /
// CalendarPager do this) and clears it on unmount. At gesture time we look up the
// deepest active route via the navigation container ref and call ITS handler.
// Routing by the live active route — rather than pushing/clearing a single
// handler on focus/blur — is robust to the native bottom-tab bar, whose JS
// focus/blur events don't fire reliably on tab switches (which left the handler
// stale or null on, e.g., the Tasks screen).
//
// The in-content native views (A11yMagicTapView / A11yPagerView) still handle the
// common case where focus is on a task/event chip; this is the fallback for
// chrome (headings / the view switcher / nav buttons — UIAccessibilityElements
// that never bubble the gesture to a mid-tree view). Only one fires per gesture:
// if an in-content view handles it, it returns YES and the gesture never reaches
// the window.

type PageDirection = 'prev' | 'next';

const magicTapActions = new Map<string, () => void>();
const pageActions = new Map<string, (direction: PageDirection) => void>();

/** Register a screen's magic-tap action under its route name. Returns an
 *  unregister function (call it on unmount). */
export function registerMagicTapAction(
  routeName: string,
  fn: () => void,
): () => void {
  magicTapActions.set(routeName, fn);
  return () => {
    if (magicTapActions.get(routeName) === fn) magicTapActions.delete(routeName);
  };
}

/** Register a screen's pager step under its route name. Returns an unregister
 *  function (call it on unmount). */
export function registerPageAction(
  routeName: string,
  fn: (direction: PageDirection) => void,
): () => void {
  pageActions.set(routeName, fn);
  return () => {
    if (pageActions.get(routeName) === fn) pageActions.delete(routeName);
  };
}

/** The deepest currently-active route name, or undefined before the container is
 *  ready. */
function activeRouteName(): string | undefined {
  return navigationRef.isReady()
    ? navigationRef.getCurrentRoute()?.name
    : undefined;
}

// Subscribe once. Android never emits these (TalkBack has no such gestures), so
// there is nothing to wire there. VoiceOver only invokes the underlying
// UIAccessibility methods when it is running, so these fire solely under a
// screen reader.
if (Platform.OS === 'ios') {
  // Both gestures are caught at the WINDOW level (swizzled onto UIWindow), so
  // they fire even while the app-lock cover — an RN Modal in the same window —
  // is up, and would act on (and speak about) the locked content behind it:
  // a magic tap on the cover would open the task editor above the lock.
  // Refuse them while the cover is on screen.
  A11yGestures.addListener('magicTap', () => {
    if (isAppLockCoverVisible()) return;
    const name = activeRouteName();
    if (name != null) magicTapActions.get(name)?.();
  });
  A11yGestures.addListener('page', (event) => {
    if (isAppLockCoverVisible()) return;
    const name = activeRouteName();
    if (name != null) pageActions.get(name)?.(event.direction);
  });
}
