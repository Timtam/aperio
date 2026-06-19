import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

export interface RadioOption<T extends string | number> {
  value: T;
  label: string;
}

/**
 * Accessible single-select. RN has no native `<select>`; for a screen-reader
 * user the faithful equivalent is a radio group where every option is its own
 * focus stop carrying `accessibilityRole="radio"` + `accessibilityState.selected`
 * (TalkBack/VoiceOver read "selected"/"not selected" and announce the change on
 * activation, so no manual announce is needed). Used for status, priority, the
 * task-list picker and the section picker.
 */
export function RadioGroup<T extends string | number>({
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  label: string;
  value: T;
  options: RadioOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  const styles = useThemedStyles(makeStyles);
  return (
    <View accessibilityRole="radiogroup" accessibilityLabel={label} style={styles.group}>
      <Text style={styles.legend}>{label}</Text>
      {options.map((opt) => {
        const selected = opt.value === value;
        return (
          <Pressable
            key={opt.value}
            accessible
            accessibilityRole="radio"
            accessibilityState={{ selected, disabled: !!disabled }}
            accessibilityLabel={opt.label}
            disabled={disabled}
            onPress={() => onChange(opt.value)}
            style={({ pressed }) => [
              styles.option,
              selected && styles.optionSelected,
              pressed && styles.optionPressed,
              disabled && styles.optionDisabled,
            ]}
          >
            <Text style={styles.marker} importantForAccessibility="no">
              {selected ? '◉' : '○'}
            </Text>
            <Text style={styles.optionLabel} importantForAccessibility="no">
              {opt.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 6 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    option: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    optionSelected: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    optionPressed: { backgroundColor: c.surfacePressed },
    optionDisabled: { opacity: 0.5 },
    marker: { fontSize: 18, width: 22, textAlign: 'center', color: c.textPrimary },
    optionLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
  });
