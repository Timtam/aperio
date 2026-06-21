import { Platform } from 'react-native';
import type { AccessibilityRole, AccessibilityState } from 'react-native';

/**
 * iOS/VoiceOver has no `radio` or `checkbox` accessibility trait — those
 * controls simply do not exist on the platform. React Native's New-Architecture
 * (Fabric) layer therefore HARDCODES the literal words "radio button" /
 * "checkbox" into the spoken value (see RCTViewComponentView.mm), so VoiceOver
 * reads e.g. "Hoch, radio button, selected" — the role word is noise for a
 * screen-reader user.
 *
 * On iOS we instead expose a single/multi-select option as a plain `button`: the
 * `accessibilityState` (`selected` / `checked`) still gives VoiceOver the
 * "selected" / "checked" wording, just without the bogus role word. On Android
 * the same roles map to real `android.widget.RadioButton` / `CheckBox` classes
 * that TalkBack announces first-class, so we keep them.
 *
 * Pass the semantic role; receive the platform-correct one. State (`selected`
 * for radio, `checked` for checkbox) stays on the element unchanged.
 */
export function selectableRole(role: 'radio' | 'checkbox'): AccessibilityRole {
  return Platform.OS === 'ios' ? 'button' : role;
}

/**
 * Multi-select state for a `checkbox` option. On iOS the option is a `button`
 * (no checkbox trait exists), and VoiceOver does NOT reliably speak the
 * `checked` state on a button — so a tapped item reads like a plain button with
 * no on/off cue ("simulated checkbox"). Use the `selected` trait instead — the
 * native iOS multi-select idiom that VoiceOver announces as "selected", exactly
 * as the radio options already do. Android keeps `checked` on its real
 * `android.widget.CheckBox`, which TalkBack announces first-class.
 */
export function selectableCheckState(checked: boolean): AccessibilityState {
  return Platform.OS === 'ios' ? { selected: checked } : { checked };
}

/**
 * Expand/collapse state for a collapsible header or row. On iOS RN renders
 * `accessibilityState.expanded` through `RCTLocalizedString("expanded" /
 * "collapsed")` — but React Native ships EMPTY localization tables
 * (`React/I18n/strings/*.lproj/Localizable.strings` are generated stubs), so the
 * lookup never resolves and VoiceOver always speaks the English fallback word,
 * no matter the app's CFBundleLocalizations or the device language. (Same class
 * of bug as the hardcoded role words above — localized in name only.) So on iOS
 * we put the ALREADY-localized word into `accessibilityValue` and drop the
 * `expanded` state; on Android `accessibilityState.expanded` is announced
 * natively in the device language, so we keep it.
 *
 * `stateWord` is the caller's translated word (e.g.
 * `t(expanded ? 'mobile.expandedState' : 'mobile.collapsedState')`); it is only
 * used on iOS.
 */
export function expandedA11y(
  expanded: boolean,
  stateWord: string,
): { accessibilityValue: { text: string } } | { accessibilityState: AccessibilityState } {
  return Platform.OS === 'ios'
    ? { accessibilityValue: { text: stateWord } }
    : { accessibilityState: { expanded } };
}
