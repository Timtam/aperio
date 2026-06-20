import { Pressable, StyleSheet, Text, View } from 'react-native';

import { selectableCheckState, selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';

export interface CheckboxOption {
  value: string;
  label: string;
}

/**
 * Accessible multi-select — the checkbox sibling of {@link RadioGroup}. Each
 * option is its own focus stop; the role comes from {@link selectableRole}
 * (native `checkbox` on Android, `button` on iOS, which has no checkbox trait)
 * while `accessibilityState.checked` carries the state — TalkBack/VoiceOver read
 * "checked"/"not checked" and announce the change on activation. An optional
 * `hint` follows the legend. Used for the search filters (calendars / task
 * lists / task statuses).
 */
export function CheckboxGroup({
  label,
  hint,
  options,
  selected,
  onToggle,
}: {
  label: string;
  hint?: string;
  options: CheckboxOption[];
  selected: Set<string>;
  onToggle: (value: string) => void;
}) {
  const styles = useThemedStyles(makeStyles);
  return (
    <View accessibilityRole="none" style={styles.group}>
      <Text style={styles.legend}>{label}</Text>
      {hint != null && <Text style={styles.hint}>{hint}</Text>}
      {options.map((opt) => {
        const checked = selected.has(opt.value);
        return (
          <Pressable
            key={opt.value}
            accessible
            accessibilityRole={selectableRole('checkbox')}
            accessibilityState={selectableCheckState(checked)}
            accessibilityLabel={opt.label}
            onPress={() => onToggle(opt.value)}
            style={({ pressed }) => [
              styles.option,
              checked && styles.optionChecked,
              pressed && styles.optionPressed,
            ]}
          >
            <Text style={styles.marker} importantForAccessibility="no">
              {checked ? '☑' : '☐'}
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
    hint: { fontSize: 13, color: c.textSecondary },
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
    optionChecked: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    optionPressed: { backgroundColor: c.surfacePressed },
    marker: { fontSize: 20, width: 24, textAlign: 'center', color: c.textPrimary },
    optionLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
  });
