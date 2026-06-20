import { SegmentedControl } from '@expo/ui/community/segmented-control';
import { useTranslation } from 'react-i18next';
import { StyleSheet, View } from 'react-native';

import type { CalendarViewKind } from './calendarViews';

// Day ⇄ Week ⇄ Month ⇄ Agenda ⇄ Year view switch — the mobile analogue of the
// desktop toolbar's view switcher (sibling calendar views, not separate tabs).
// A native SegmentedControl: a real UISegmentedControl on iOS (SwiftUI Picker,
// .segmented) and a Material SingleChoiceSegmentedButtonRow on Android, so
// VoiceOver/TalkBack get native segmented-control semantics. Selecting the
// already-active view is a no-op.

export type { CalendarViewKind };

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
    { key: 'month', label: t('toolbar.views.month') },
    { key: 'agenda', label: t('toolbar.views.agenda') },
    { key: 'year', label: t('toolbar.views.year') },
  ];
  const selectedIndex = Math.max(
    0,
    options.findIndex((o) => o.key === active),
  );
  return (
    <View accessibilityLabel={t('toolbar.viewSwitch')} style={styles.row}>
      <SegmentedControl
        values={options.map((o) => o.label)}
        selectedIndex={selectedIndex}
        onChange={(e) => {
          const opt = options[e.nativeEvent.selectedSegmentIndex];
          if (opt && opt.key !== active) onSelect(opt.key);
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  row: { paddingHorizontal: 12, paddingTop: 12 },
});
