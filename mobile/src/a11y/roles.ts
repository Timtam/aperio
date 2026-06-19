import { Platform } from 'react-native';
import type { AccessibilityRole } from 'react-native';

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
