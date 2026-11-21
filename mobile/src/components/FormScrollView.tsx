import { useContext, useEffect, useState, type ReactNode } from 'react';
import {
  Keyboard,
  LayoutAnimation,
  Platform,
  ScrollView,
  StyleSheet,
  useWindowDimensions,
  View,
  type KeyboardEvent,
  type StyleProp,
  type ViewStyle,
} from 'react-native';
import { HeaderHeightContext } from '@react-navigation/elements';

// Form scroll container that keeps a focused TextInput — and the VoiceOver
// cursor on it — stable when the keyboard opens. A bare <ScrollView> lets iOS
// re-inset / re-layout the moment the keyboard appears, which can drop the
// input's focus and bounce the VoiceOver cursor into the view behind (the
// app-wide "focus jumps as soon as a field is focused" report). So the form
// avoids the keyboard itself, with bottom padding, and the scroll view does
// not fight it. Drop-in replacement for the editors' root ScrollView.
//
// The padding is the part of the form the keyboard covers, in WINDOW
// coordinates: the form's distance from the top of the window, plus its
// height, minus the keyboard's top. On a pushed screen that distance is the
// header height. In an editor shown as a sheet (presentation 'modal', a page
// sheet on iPhone) the sheet itself starts some 70 points down the screen, and
// the header height leaves that out: the bottom strip of the form stayed
// behind the keyboard, where VoiceOver neither scrolled to its fields nor
// reached them by swiping (Toni, build 74: Priority and Effort in a new task,
// keyboard open). Every form here reaches the bottom of the window on an
// iPhone, so the window height minus the form's own height IS its distance
// from the top. An iPad centres its sheets, so there the header height stays.
//
// Not React Native's KeyboardAvoidingView: it computes the padding only when
// the keyboard or its own height changes, with the offset it had then. A new
// task focuses its title as the sheet comes up, so the keyboard can be
// reported before the sheet's final layout, and the padding kept the stale
// offset. Here the padding is worked out on every render from the latest
// keyboard top and the latest layout, whichever came first.
//
// Android resizes the window for the keyboard itself and gets no padding, as
// before.

export function FormScrollView({
  style,
  contentContainerStyle,
  accessibilityViewIsModal,
  children,
}: {
  style?: StyleProp<ViewStyle>;
  contentContainerStyle?: StyleProp<ViewStyle>;
  accessibilityViewIsModal?: boolean;
  children: ReactNode;
}) {
  // From the context, not useHeaderHeight(): that throws outside a stack
  // screen, and CloseOnlyNotice renders this inside the day-start review's
  // Modal, which has no header.
  const headerHeight = useContext(HeaderHeightContext) ?? 0;
  const { height: windowHeight } = useWindowDimensions();
  const [frame, setFrame] = useState<{ y: number; height: number } | null>(null);
  const [keyboardTop, setKeyboardTop] = useState<number | null>(() =>
    Platform.OS === 'ios' && Keyboard.isVisible()
      ? (Keyboard.metrics()?.screenY ?? null)
      : null,
  );

  useEffect(() => {
    if (Platform.OS !== 'ios') return;
    const follow = (event: KeyboardEvent, top: number | null) => {
      // Move with the keyboard, as KeyboardAvoidingView does (10 ms is the
      // shortest duration a layout animation takes).
      if (event.duration > 0) {
        const duration = Math.max(event.duration, 10);
        LayoutAnimation.configureNext({
          duration,
          update: {
            duration,
            type: LayoutAnimation.Types[event.easing] ?? LayoutAnimation.Types.keyboard,
          },
        });
      }
      setKeyboardTop(top);
    };
    const subscriptions = [
      // With Prefer Cross-Fade Transitions iOS reports the keyboard at y 0;
      // KeyboardAvoidingView pads nothing then, and neither does this.
      Keyboard.addListener('keyboardWillShow', (e) =>
        follow(e, e.endCoordinates.screenY > 0 ? e.endCoordinates.screenY : null),
      ),
      Keyboard.addListener('keyboardWillHide', (e) => follow(e, null)),
    ];
    return () => subscriptions.forEach((s) => s.remove());
  }, []);

  const fromWindowTop =
    Platform.OS === 'ios' && !Platform.isPad && frame != null
      ? Math.max(headerHeight, windowHeight - frame.height)
      : headerHeight;
  const covered =
    keyboardTop != null && frame != null
      ? Math.max(0, frame.y + frame.height + fromWindowTop - keyboardTop)
      : 0;

  return (
    <View
      accessibilityViewIsModal={accessibilityViewIsModal}
      // The padding changes what is inside, not this view's own frame, so
      // measuring it here cannot feed back into the padding.
      onLayout={(e) => {
        const { y, height } = e.nativeEvent.layout;
        setFrame((old) => (old?.y === y && old.height === height ? old : { y, height }));
      }}
      style={[styles.flex, style, Platform.OS === 'ios' ? { paddingBottom: covered } : null]}
    >
      <ScrollView
        style={styles.flex}
        contentContainerStyle={contentContainerStyle}
        keyboardShouldPersistTaps="handled"
      >
        {children}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({ flex: { flex: 1 } });
