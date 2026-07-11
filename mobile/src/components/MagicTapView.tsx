import { type ReactNode } from 'react';
import {
  Platform,
  StyleSheet,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';

import { A11yMagicTapView } from '../../modules/a11y-magic-tap';

// Wraps a screen's content so a VoiceOver MAGIC TAP (two-finger double-tap) runs
// the screen's single most relevant action — creating its primary item (a new
// event / task / contact) — from anywhere on the screen, no matter where focus
// sits.
//
// React Native's own `onMagicTap` prop no-ops on the New Architecture (Fabric
// never receives it — see modules/a11y-magic-tap), so on iOS we render the native
// A11yMagicTapView, which overrides `accessibilityPerformMagicTap` and catches the
// gesture as it bubbles up the responder chain from the focused element. The
// native view stays a transparent flex container; the screen's real styled root
// (background + flex) is a normal RN View INSIDE it, so the sighted appearance is
// unchanged and background colour is guaranteed to render.
//
// On Android — where TalkBack has no magic-tap gesture — this is just the plain
// styled View, so nothing changes there.

export function MagicTapView({
  style,
  onMagicTap,
  children,
}: {
  style?: StyleProp<ViewStyle>;
  onMagicTap: () => void;
  children: ReactNode;
}) {
  if (Platform.OS === 'ios') {
    return (
      <A11yMagicTapView style={styles.fill} onMagicTap={() => onMagicTap()}>
        <View style={style}>{children}</View>
      </A11yMagicTapView>
    );
  }
  return <View style={style}>{children}</View>;
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
});
