import { useIsFocused } from '@react-navigation/native';

/**
 * Accessibility props that make a screen's content inert (hidden from the screen
 * reader) while the screen is NOT the focused route. Spread onto the screen's
 * root View/ScrollView.
 *
 * Why: with native-stack + react-native-screens, an INTERRUPTED back-swipe
 * (start navigating, then swipe back mid-transition) momentarily leaves both the
 * dismissing screen and the revealed screen in the accessibility tree, so
 * VoiceOver/TalkBack can snap onto a stale descendant of the not-yet-focused
 * screen — e.g. the Settings hub's last "Logs" row — then get yanked away when
 * the transition settles. Driving accessibility off `useIsFocused()` keeps the
 * off-screen screen out of the a11y tree until it is actually focused, the
 * mobile twin of the desktop's `inert`/`aria-hidden` on unfocused panels. Do NOT
 * "fix" this with a setAccessibilityFocus call — that fights the screen reader.
 */
export function useScreenA11yInert(): {
  accessibilityElementsHidden: boolean;
  importantForAccessibility: 'auto' | 'no-hide-descendants';
} {
  const focused = useIsFocused();
  return {
    // iOS / VoiceOver:
    accessibilityElementsHidden: !focused,
    // Android / TalkBack:
    importantForAccessibility: focused ? 'auto' : 'no-hide-descendants',
  };
}
