import { useEffect, useState } from 'react';
import { AccessibilityInfo, AppState } from 'react-native';

/**
 * Whether a screen reader (VoiceOver / TalkBack) is currently running.
 *
 * Seeds from the one-shot `isScreenReaderEnabled()`, stays live via the
 * `screenReaderChanged` event, and RE-PROBES whenever the app comes back to the
 * foreground. Used by {@link CalendarPager} to drop the swipe-to-page ScrollView
 * under VoiceOver (its spacer pages announce "page 1 of 3" and steal focus out
 * of the view).
 *
 * The re-probe is not belt-and-braces, it is the fix for a real failure. The
 * event only fires on a CHANGE, so for someone who has VoiceOver on ALL the
 * time it never fires at all — and if the one-shot probe races the launch and
 * answers false, the hook stays false for the whole session. The pager then
 * renders its sighted three-page ScrollView, a three-finger swipe lands on a
 * spacer, and iOS announces "page 1 of 3" and refuses to go further: exactly
 * the symptom, and permanent until the app is restarted into a luckier race.
 *
 * Foregrounding is the cheapest moment that reliably recurs, and it also covers
 * the reader being switched on while the app was in the background — another
 * change the event does not deliver.
 */
export function useScreenReaderEnabled(): boolean {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    let active = true;
    const probe = () => {
      void AccessibilityInfo.isScreenReaderEnabled().then((on) => {
        if (active) setEnabled(on);
      });
    };
    probe();
    const changed = AccessibilityInfo.addEventListener(
      'screenReaderChanged',
      (on: boolean) => setEnabled(on),
    );
    const app = AppState.addEventListener('change', (state) => {
      if (state === 'active') probe();
    });
    return () => {
      active = false;
      changed.remove();
      app.remove();
    };
  }, []);
  return enabled;
}
