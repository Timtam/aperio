import { type ReactNode } from 'react';
import {
  KeyboardAvoidingView,
  Platform,
  ScrollView,
  StyleSheet,
  type StyleProp,
  type ViewStyle,
} from 'react-native';
import { useHeaderHeight } from '@react-navigation/elements';

// Form scroll container that keeps a focused TextInput — and the VoiceOver
// cursor on it — stable when the keyboard opens. A bare <ScrollView> lets iOS
// re-inset / re-layout the moment the keyboard appears, which can drop the
// input's focus and bounce the VoiceOver cursor into the view behind (the
// app-wide "focus jumps as soon as a field is focused" report). A
// KeyboardAvoidingView owns the avoidance instead, so the scroll view doesn't
// fight the keyboard. Drop-in replacement for the editors' root ScrollView; the
// header height feeds keyboardVerticalOffset so the avoided area accounts for
// the (modal) nav header.

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
  const headerHeight = useHeaderHeight();
  return (
    <KeyboardAvoidingView
      accessibilityViewIsModal={accessibilityViewIsModal}
      style={[styles.flex, style]}
      behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      keyboardVerticalOffset={headerHeight}
    >
      <ScrollView
        style={styles.flex}
        contentContainerStyle={contentContainerStyle}
        keyboardShouldPersistTaps="handled"
      >
        {children}
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({ flex: { flex: 1 } });
