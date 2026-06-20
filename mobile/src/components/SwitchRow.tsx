import { Pressable, StyleSheet, Switch, Text, View } from 'react-native';

import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Accessible on/off row: a single `switch`-role Pressable owns the toggle (the
// inner Switch is a visual indicator only, hidden from the reader). VoiceOver /
// TalkBack announce the on/off state + the toggle action first-class, since
// `switch` is a real native trait on both platforms. An optional hint follows.

export function SwitchRow({
  label,
  hint,
  value,
  onToggle,
  disabled,
}: {
  label: string;
  hint?: string;
  value: boolean;
  onToggle: () => void;
  disabled?: boolean;
}) {
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  return (
    <View style={styles.group}>
      <Pressable
        accessibilityRole="switch"
        accessibilityState={{ checked: value, disabled }}
        accessibilityLabel={label}
        accessibilityHint={hint}
        disabled={disabled}
        onPress={onToggle}
        style={({ pressed }) => [styles.row, pressed && styles.pressed]}
      >
        <Text style={styles.label} importantForAccessibility="no">
          {label}
        </Text>
        <View pointerEvents="none">
          <Switch
            value={value}
            trackColor={{ false: colors.border, true: colors.accent }}
            importantForAccessibility="no"
            accessibilityElementsHidden
          />
        </View>
      </Pressable>
      {hint != null && (
        <Text style={styles.hint} importantForAccessibility="no">
          {hint}
        </Text>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 6 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    pressed: { opacity: 0.7 },
    label: { flex: 1, fontSize: 16, color: c.textPrimary },
    hint: { fontSize: 13, color: c.textSecondary },
  });
