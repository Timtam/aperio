import { Platform } from 'react-native';

import { A11yGestures } from '../../modules/a11y-gestures';

// Routes the app-wide, window-level VoiceOver gestures (magic tap + horizontal
// three-finger scroll, caught natively — see modules/a11y-gestures) to the
// CURRENTLY FOCUSED screen. The focused screen registers its handlers on focus
// (MagicTapView / CalendarPager do this via useFocusEffect) and clears them on
// blur, so a gesture that lands on the window — because VoiceOver focus was on a
// heading / chrome control the in-content native views can't catch — still runs
// the right screen's action.
//
// The in-content native views (A11yMagicTapView / A11yPagerView) still handle the
// common case where focus is on a task/event chip; this is the fallback for
// everything else. Only one fires per gesture: if an in-content view handles it,
// it returns YES and the gesture never reaches the window.

type PageDirection = 'prev' | 'next';

let magicTapAction: (() => void) | null = null;
let pageAction: ((direction: PageDirection) => void) | null = null;

/** Register the focused screen's magic-tap action (its primary create action);
 *  pass `null` on blur. */
export function setMagicTapAction(fn: (() => void) | null): void {
  magicTapAction = fn;
}

/** Register the focused screen's pager step (prev/next); pass `null` on blur. */
export function setPageAction(
  fn: ((direction: PageDirection) => void) | null,
): void {
  pageAction = fn;
}

// Subscribe once. Android never emits these (TalkBack has no such gestures), so
// there is nothing to wire there. VoiceOver only invokes the underlying
// UIAccessibility methods when it is running, so these fire solely under a
// screen reader.
if (Platform.OS === 'ios') {
  A11yGestures.addListener('magicTap', () => {
    magicTapAction?.();
  });
  A11yGestures.addListener('page', (event) => {
    pageAction?.(event.direction);
  });
}
