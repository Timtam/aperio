import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

// Day ⇄ Week ⇄ Agenda view switch — the mobile analogue of the desktop toolbar's
// view switcher (sibling calendar views, not separate tabs). Buttons carry
// accessibilityState.selected (TalkBack/VoiceOver announce "selected") + a
// visible selected style for sighted users. Pressing the active view is a no-op.

export type CalendarViewKind = 'day' | 'week' | 'agenda';

export function CalendarViewSwitcher({
  active,
  onSelect,
}: {
  active: CalendarViewKind;
  onSelect: (view: CalendarViewKind) => void;
}) {
  const { t } = useTranslation();
  const options: { key: CalendarViewKind; label: string }[] = [
    { key: 'day', label: t('toolbar.views.day') },
    { key: 'week', label: t('toolbar.views.week') },
    { key: 'agenda', label: t('toolbar.views.agenda') },
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

const styles = StyleSheet.create({
  row: { flexDirection: 'row', gap: 8, paddingHorizontal: 12, paddingTop: 12 },
  button: {
    flex: 1,
    paddingVertical: 10,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
    alignItems: 'center',
  },
  buttonSelected: { borderColor: '#1d4ed8', backgroundColor: '#eaf0fd' },
  buttonText: { fontSize: 16, fontWeight: '600', color: '#1d3a2f' },
  buttonTextSelected: { color: '#1d3a2f', fontWeight: '700' },
  pressed: { backgroundColor: '#e4ebf5' },
});
