import { Platform, StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import { NamedSegmentedControl } from './NamedSegmentedControl';

export interface SegmentedOption<T extends string | number> {
  value: T;
  label: string;
}

/**
 * Native single-select for SMALL, fixed option sets (task priority and effort,
 * the move/copy choices). A thin wrapper over {@link NamedSegmentedControl}: a
 * real UISegmentedControl on iOS and a Material SingleChoiceSegmentedButtonRow
 * on Android, so VoiceOver/TalkBack get genuinely native segmented-control
 * semantics instead of RN's synthesized "radio button" text. For long or
 * variable option lists use {@link RadioGroup} instead: a segmented control can
 * only hold a handful of segments.
 *
 * Its name: on iOS the control carries the legend as a named container, which
 * VoiceOver speaks as focus enters it, so the visible legend is hidden from
 * VoiceOver and the field is one stop. Android's control takes no name, so
 * there the legend Text stays a stop of its own before it (as in RadioGroup).
 * Selection is mapped by index (robust against two options sharing a label).
 */
export function SegmentedSelect<T extends string | number>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: SegmentedOption<T>[];
  onChange: (value: T) => void;
}) {
  const styles = useThemedStyles(makeStyles);
  const selectedIndex = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  );
  return (
    <View style={styles.group}>
      <Text style={styles.legend} aria-hidden={Platform.OS === 'ios'}>
        {label}
      </Text>
      <NamedSegmentedControl
        name={label}
        values={options.map((o) => o.label)}
        selectedIndex={selectedIndex}
        onSelect={(index) => {
          const opt = options[index];
          if (opt) onChange(opt.value);
        }}
      />
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 6 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
  });
