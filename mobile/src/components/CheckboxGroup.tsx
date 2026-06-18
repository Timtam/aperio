import { Pressable, StyleSheet, Text, View } from 'react-native';

export interface CheckboxOption {
  value: string;
  label: string;
}

/**
 * Accessible multi-select — the checkbox sibling of {@link RadioGroup}. Each
 * option is its own focus stop carrying `accessibilityRole="checkbox"` +
 * `accessibilityState.checked` (TalkBack/VoiceOver read "checked"/"not checked"
 * and announce the change on activation). An optional `hint` follows the legend.
 * Used for the search filters (calendars / task lists / task statuses).
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
            accessibilityRole="checkbox"
            accessibilityState={{ checked }}
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

const styles = StyleSheet.create({
  group: { gap: 6 },
  legend: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  hint: { fontSize: 13, color: '#5b6573' },
  option: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  optionChecked: { borderColor: '#1d4ed8', backgroundColor: '#eaf0fd' },
  optionPressed: { backgroundColor: '#e4ebf5' },
  marker: { fontSize: 20, width: 24, textAlign: 'center', color: '#10131a' },
  optionLabel: { flex: 1, fontSize: 16, color: '#10131a' },
});
