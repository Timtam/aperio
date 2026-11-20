import { SegmentedControl } from '@expo/ui/community/segmented-control';
import { Platform, StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

export interface SegmentedOption<T extends string | number> {
  value: T;
  label: string;
}

/**
 * Native single-select for SMALL, fixed option sets (task status, priority, the
 * calendar view switch). A thin wrapper over @expo/ui's SegmentedControl, which
 * renders a real UISegmentedControl (SwiftUI Picker, .segmented) on iOS and a
 * Material SingleChoiceSegmentedButtonRow on Android — so VoiceOver/TalkBack get
 * genuinely native segmented-control semantics instead of RN's synthesized
 * "radio button" text. For long or variable option lists use {@link RadioGroup}
 * instead: a segmented control can only hold a handful of segments.
 *
 * On iOS the legend is the control's own name (@expo/ui 58's
 * `accessibilityLabel`, a SwiftUI label), and the visible legend is hidden from
 * VoiceOver so it is not read twice. Android's control takes no label, so there
 * the legend Text stays a stop of its own before it (as in RadioGroup).
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
      <SegmentedControl
        accessibilityLabel={label}
        values={options.map((o) => o.label)}
        selectedIndex={selectedIndex}
        onChange={(e) => {
          const opt = options[e.nativeEvent.selectedSegmentIndex];
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
