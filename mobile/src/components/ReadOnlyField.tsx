import { useTranslation } from 'react-i18next';
import { StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * One field the user can read but not change: an invitation somebody else
 * organizes (decision 77a).
 *
 * One stop for the screen reader, reading "label: value" — the row carries
 * the whole sentence and its two Texts are hidden from the reader, so a field
 * is one swipe and not three. Both hiding props are set: `accessibilityElementsHidden`
 * is iOS, `importantForAccessibility` Android, and setting only one leaves
 * the other platform reading the value twice.
 *
 * Never a disabled or `editable={false}` TextInput: iOS then reads English
 * words into a German app, and a disabled control is skipped by the reading
 * cursor — the value is the whole point here.
 */
export function ReadOnlyField({ label, value }: { label: string; value: string }) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  return (
    <View
      accessible
      accessibilityRole="text"
      accessibilityLabel={t('dialogs.readOnlyField', { label, value })}
      style={styles.field}
    >
      <Text style={styles.label} accessibilityElementsHidden importantForAccessibility="no">
        {label}
      </Text>
      <Text
        style={styles.value}
        selectable
        accessibilityElementsHidden
        importantForAccessibility="no"
      >
        {value}
      </Text>
    </View>
  );
}

const makeStyles = (colors: ThemeColors) =>
  StyleSheet.create({
    field: { marginBottom: 16 },
    label: { color: colors.textLabel, fontSize: 15, fontWeight: '600', marginBottom: 4 },
    value: {
      color: colors.textPrimary,
      fontSize: 17,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: StyleSheet.hairlineWidth,
      borderColor: colors.border,
      backgroundColor: colors.surfaceSubtle,
    },
  });
