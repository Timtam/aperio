import { useEffect, useState } from 'react';
import { AccessibilityInfo } from 'react-native';

/**
 * Whether a screen reader (VoiceOver / TalkBack) is currently running.
 *
 * Seeds from the one-shot `isScreenReaderEnabled()` and stays live via the
 * `screenReaderChanged` event, so a component can adapt its interaction model
 * when the reader is toggled mid-session. Used by {@link CalendarPager} to drop
 * the swipe-to-page ScrollView under VoiceOver (its spacer pages announce
 * "page 1 of 3" and steal focus out of the view).
 */
export function useScreenReaderEnabled(): boolean {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    let active = true;
    void AccessibilityInfo.isScreenReaderEnabled().then((on) => {
      if (active) setEnabled(on);
    });
    const sub = AccessibilityInfo.addEventListener(
      'screenReaderChanged',
      (on: boolean) => setEnabled(on),
    );
    return () => {
      active = false;
      sub.remove();
    };
  }, []);
  return enabled;
}
