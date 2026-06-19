import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import type { CalendarViewKind } from './calendarViews';

// Day ⇄ Week ⇄ Month ⇄ Agenda ⇄ Year view switch — the mobile analogue of the
// desktop toolbar's view switcher (sibling calendar views, not separate tabs).
// Buttons carry accessibilityState.selected (TalkBack/VoiceOver announce
// "selected") + a visible selected style for sighted users. Pressing the active
// view is a no-op.

export type { CalendarViewKind };

export function CalendarViewSwitcher({
  active,
  onSelect,
}: {
  active: CalendarViewKind;
  onSelect: (view: CalendarViewKind) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const options: { key: CalendarViewKind; label: string }[] = [
    { key: 'day', label: t('toolbar.views.day') },
    { key: 'week', label: t('toolbar.views.week') },
    { key: 'month', label: t('toolbar.views.month') },
    { key: 'agenda', label: t('toolbar.views.agenda') },
    { key: 'year', label: t('toolbar.views.year') },
  ];
  return (
    <View
      accessibilityRole="tablist"
      accessibilityLabel={t('toolbar.viewSwitch')}
      style={styles.row}
    >
      {options.map((opt) => {
        const selected = opt.key === active;
        return (
          <Pressable
            key={opt.key}
            accessibilityRole="button"
            accessibilityState={{ selected }}
            accessibilityLabel={opt.label}
            onPress={() => {
              if (!selected) onSelect(opt.key);
            }}
            style={({ pressed }) => [
              styles.button,
              selected && styles.buttonSelected,
              pressed && styles.pressed,
            ]}
          >
            <Text
              style={[styles.buttonText, selected && styles.buttonTextSelected]}
              importantForAccessibility="no"
            >
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
    row: { flexDirection: 'row', gap: 8, paddingHorizontal: 12, paddingTop: 12 },
    button: {
      flex: 1,
      paddingVertical: 10,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    buttonSelected: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.link },
    buttonTextSelected: { color: c.link, fontWeight: '700' },
    pressed: { backgroundColor: c.surfacePressed },
  });
