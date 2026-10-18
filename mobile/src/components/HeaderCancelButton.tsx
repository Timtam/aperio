import { Pressable, StyleSheet, Text } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

// A Cancel button for a modal screen's header-left — the first element a
// screen-reader user reaches, so they can back out of an editor without swiping
// through the whole form (the back-swipe works too, but isn't discoverable).
// Wire via navigation.setOptions({ headerLeft: () => <HeaderCancelButton … /> }).
// `disabled` while the screen may not be left (a write runs): it stays in
// place and says it is unavailable.

export function HeaderCancelButton({
  label,
  onPress,
  disabled = false,
}: {
  label: string;
  onPress: () => void;
  disabled?: boolean;
}) {
  const styles = useThemedStyles(makeStyles);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      hitSlop={10}
      style={({ pressed }) => [
        styles.button,
        pressed && styles.pressed,
        disabled && styles.disabled,
      ]}
    >
      <Text style={styles.text}>{label}</Text>
    </Pressable>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    button: { paddingVertical: 4, paddingHorizontal: 4 },
    pressed: { opacity: 0.6 },
    disabled: { opacity: 0.4 },
    text: { fontSize: 17, fontWeight: '600', color: c.link },
  });
