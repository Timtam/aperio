import { Pressable, StyleSheet, Text, View } from 'react-native';

import { selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';

export interface RadioOption<T extends string | number> {
  value: T;
  label: string;
}

/**
 * Accessible single-select. RN has no native `<select>`; for a screen-reader
 * user the faithful equivalent is a radio group where every option is its own
 * focus stop. The per-option role comes from {@link selectableRole} (native
 * `radio` on Android, `button` on iOS, which has no radio trait) while
 * `accessibilityState.selected` carries the choice; TalkBack/VoiceOver announce
 * the change on activation, so no manual announce is needed. Used for status,
 * priority, the task-list picker and the section picker.
 */
export function RadioGroup<T extends string | number>({
  label,
  value,
  options,
  onChange,
  disabled,
  labelAsHeading,
}: {
  label: string;
  value: T;
  options: RadioOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  /** Expose the legend as an accessibility heading, so screen-reader heading
   *  navigation reaches section-level pickers (the Settings groups). Off by
   *  default — inline pickers (status/priority/list/section) are not headers. */
  labelAsHeading?: boolean;
}) {
  const styles = useThemedStyles(makeStyles);
  return (
    <View accessibilityRole="radiogroup" accessibilityLabel={label} style={styles.group}>
      <Text style={styles.legend} accessibilityRole={labelAsHeading ? 'header' : undefined}>
        {label}
      </Text>
      {options.map((opt) => {
        const selected = opt.value === value;
        return (
          <Pressable
            key={opt.value}
            accessible
            accessibilityRole={selectableRole('radio')}
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
