import { useTranslation } from 'react-i18next';
import { Platform, StyleSheet, View } from 'react-native';

import type { CalendarViewKind } from './calendarViews';
import { NamedSegmentedControl } from './NamedSegmentedControl';

// Day ⇄ Week ⇄ Month ⇄ Agenda ⇄ Year view switch — the mobile analogue of the
// desktop toolbar's view switcher (sibling calendar views, not separate tabs).
// A native SegmentedControl: a real UISegmentedControl on iOS (SwiftUI Picker,
// .segmented) and a Material SingleChoiceSegmentedButtonRow on Android, so
// VoiceOver/TalkBack get native segmented-control semantics. Selecting the
// already-active view is a no-op.
//
// Its name: on iOS the control is a named container, which VoiceOver speaks as
// focus enters it (NamedSegmentedControl.ios.tsx). A label on the plain
// wrapper View never reached VoiceOver, since iOS reads no label from a view
// that is not an accessibility element. Android has no name on the control,
// and keeps the wrapper's.

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
    <View
      accessibilityLabel={Platform.OS === 'android' ? t('toolbar.viewSwitch') : undefined}
      style={styles.row}
    >
      <NamedSegmentedControl
        name={t('toolbar.viewSwitch')}
        values={options.map((o) => o.label)}
        selectedIndex={selectedIndex}
        onSelect={(index) => {
          const opt = options[index];
          if (opt && opt.key !== active) onSelect(opt.key);
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  row: { paddingHorizontal: 12, paddingTop: 12 },
});
