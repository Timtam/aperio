import { Pressable, StyleSheet, Text } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

// A Cancel button for a modal screen's header-left — the first element a
// screen-reader user reaches, so they can back out of an editor without swiping
// through the whole form (the back-swipe works too, but isn't discoverable).
// Wire via navigation.setOptions({ headerLeft: () => <HeaderCancelButton … /> }).

export function HeaderCancelButton({
  label,
  onPress,
}: {
  label: string;
  onPress: () => void;
}) {
  const styles = useThemedStyles(makeStyles);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      hitSlop={10}
      style={({ pressed }) => [styles.button, pressed && styles.pressed]}
    >
      <Text style={styles.text}>{label}</Text>
    </Pressable>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    button: { paddingVertical: 4, paddingHorizontal: 4 },
    pressed: { opacity: 0.6 },
    text: { fontSize: 17, fontWeight: '600', color: c.link },
  });
