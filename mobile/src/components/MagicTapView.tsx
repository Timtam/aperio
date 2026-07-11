import { useFocusEffect } from '@react-navigation/native';
import { useCallback, type ReactNode } from 'react';
import {
  Platform,
  StyleSheet,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';

import { A11yMagicTapView } from '../../modules/a11y-magic-tap';
import { setMagicTapAction } from '../a11y/gestureHost';

// Wraps a screen's content so a VoiceOver MAGIC TAP (two-finger double-tap) runs
// the screen's single most relevant action — creating its primary item (a new
// event / task / contact) — from anywhere on the screen, no matter where focus
// sits.
//
// React Native's own `onMagicTap` prop no-ops on the New Architecture (Fabric
// never receives it — see modules/a11y-magic-tap), so on iOS we render the native
// A11yMagicTapView, which overrides `accessibilityPerformMagicTap` and catches the
// gesture as it bubbles up the responder chain from a focused REAL-view element (a
// task/event chip). The native view stays a transparent flex container; the
// screen's real styled root (background + flex) is a normal RN View INSIDE it, so
// the sighted appearance is unchanged.
//
// But when focus is on a `UIAccessibilityElement` (a heading / chrome control that
// is not a UIView responder), the magic tap never reaches a mid-tree view — it
// ends up at the window instead. So we ALSO register this screen's action in the
// app-wide gesture host (src/a11y/gestureHost.ts) while the screen is focused; the
// window-level native catcher (modules/a11y-gestures) routes those to it. Only one
// path fires per gesture (an in-content hit stops the chain before the window).
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
  // Window-level fallback: keep this screen's action current while it's focused.
  useFocusEffect(
    useCallback(() => {
      setMagicTapAction(onMagicTap);
      return () => setMagicTapAction(null);
    }, [onMagicTap]),
  );

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
