import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  findNodeHandle,
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { useTranslation } from 'react-i18next';

import {
  delayedSay,
  listedZoneLabels,
  unusableZones,
  zoneGroups,
  zoneOffsets,
  zonePickerList,
  zoneRegionNames,
  type ZonePick,
} from '@aperio/shared';

import { selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';

/** How long the typing must pause before iOS hears the count line (122b);
 *  Android reads it from the live region. */
const COUNT_PAUSE_MS = 800;

/** The zones the series' calendar stores, in the manifest's form (97a). */
export type CalendarZones = 'all' | 'none' | string[];

/**
 * The series time zone picker on the phone (DESIGN-series-time-zone.md, stage
 * 11, "Handy-Dialog"): a dialog of its own with its title, the search field
 * with the first focus, the count line, the rows and „Abbrechen“. A tapped row
 * is taken and the dialog closes; a row that cannot be chosen says why in its
 * label and does nothing. The rows are the shared `zonePickerList`, the same
 * the desktop shows.
 *
 * Mount it in the editor's own screen, never through a host at the app root:
 * a root modal does not appear over the editor sheet on iOS. The result is
 * handed over once the dialog is gone (iOS `onDismiss`, Android the next
 * turn), so the caller can open the weekday question at once.
 *
 * Unmeasured on a device (DESIGN "Ungeprüft"): VoiceOver and TalkBack with the
 * 62 groups and up to 313 cities, and whether the count line is heard.
 */
export function ZonePickerDialog({
  startAt,
  today,
  stored,
  deviceZone,
  calendarZones,
  onDone,
}: {
  /** The series' start, RFC 3339: a city shows its offset then. */
  startAt: string;
  /** Today, RFC 3339, from the phone's clock. */
  today: string;
  /** The zone the series stores, as stored; `null` for none. */
  stored: string | null;
  /** The device's zone name as the runtime reports it (`deviceZoneName`). */
  deviceZone: string | null;
  calendarZones: CalendarZones;
  /** The choice once the dialog is gone; `null` when it was cancelled. */
  onDone: (pick: ZonePick | null) => void;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const inputRef = useRef<TextInput | null>(null);
  const [visible, setVisible] = useState(true);
  const [query, setQuery] = useState('');
  const pending = useRef<ZonePick | null>(null);
  const done = useRef(false);
  const typed = useRef(false);

  const sayCount = useMemo(
    () => delayedSay((text) => AccessibilityInfo.announceForAccessibility(text), COUNT_PAUSE_MS),
    [],
  );
  useEffect(() => () => sayCount.cancel(), [sayCount]);

  const calendarKey = JSON.stringify(calendarZones);
  const groups = useMemo(
    () =>
      zoneGroups({
        today,
        stored,
        device: deviceZone,
        calendar_zones: JSON.parse(calendarKey) as CalendarZones,
        unusable: unusableZones(
          listedZoneLabels().map((label) => label.zone),
          stored,
        ),
      }),
    [today, stored, deviceZone, calendarKey],
  );
  const offsets = useMemo(() => zoneOffsets(startAt, today), [startAt, today]);
  const list = useMemo(() => {
    const language = i18n.language;
    const formatDay = (day: string) =>
      new Intl.DateTimeFormat(language, {
        day: 'numeric',
        month: 'long',
        year: 'numeric',
        timeZone: 'UTC',
      }).format(new Date(`${day}T00:00:00Z`));
    return zonePickerList({
      query,
      groups,
      offsets,
      regionNames: zoneRegionNames(t),
      stored,
      device: deviceZone,
      t,
      formatDay,
    });
  }, [groups, offsets, query, stored, deviceZone, t, i18n.language]);

  // iOS hears the count line once the typing pauses; Android reads it from
  // the live region. Never for the list as it opens, where it is the search
  // field's hint.
  useEffect(() => {
    if (Platform.OS === 'ios' && typed.current) {
      typed.current = false;
      sayCount.schedule(list.count);
    }
  }, [list, sayCount]);

  // The result, once the dialog is gone: iOS refuses to present another
  // dialog (the weekday question) while this one is still dismissing.
  const finish = () => {
    if (done.current) return;
    done.current = true;
    onDone(pending.current);
  };
  const close = (pick: ZonePick | null) => {
    sayCount.cancel();
    pending.current = pick;
    setVisible(false);
    if (Platform.OS !== 'ios') setTimeout(finish, 0);
  };

  const focusSearch = () => {
    const tag = findNodeHandle(inputRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  };

  return (
    <Modal
      visible={visible}
      animationType="slide"
      onRequestClose={() => close(null)}
      onShow={focusSearch}
      onDismiss={Platform.OS === 'ios' ? finish : undefined}
    >
      <View style={styles.root} accessibilityViewIsModal>
        <Text style={styles.title} accessibilityRole="header">
          {t('dialogs.seriesTimeZone.title')}
        </Text>
        <TextInput
          ref={inputRef}
          style={styles.input}
          value={query}
          onChangeText={(text) => {
            typed.current = true;
            setQuery(text);
          }}
          accessibilityLabel={t('dialogs.seriesTimeZone.searchLabel')}
          accessibilityHint={list.count}
          autoCapitalize="none"
          autoCorrect={false}
          returnKeyType="search"
        />
        <Text style={styles.count} accessibilityLiveRegion="polite">
          {list.count}
        </Text>
        <ScrollView
          style={styles.list}
          contentContainerStyle={styles.listContent}
          keyboardShouldPersistTaps="handled"
          automaticallyAdjustKeyboardInsets
          accessibilityLabel={t('dialogs.seriesTimeZone.listLabel')}
        >
          {list.rows.map((row) => (
            <Pressable
              key={row.key}
              accessible
              accessibilityRole={selectableRole('radio')}
              accessibilityState={{ selected: row.current }}
              accessibilityLabel={row.text}
              onPress={() => {
                if (row.selectable) close(row.pick);
              }}
              style={({ pressed }) => [
                styles.row,
                row.current && styles.rowCurrent,
                !row.selectable && styles.rowRefused,
                pressed && row.selectable && styles.rowPressed,
              ]}
            >
              <Text style={styles.marker} importantForAccessibility="no">
                {row.current ? '◉' : '○'}
              </Text>
              <Text style={styles.rowText} importantForAccessibility="no">
                {row.text}
              </Text>
            </Pressable>
          ))}
        </ScrollView>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.cancel')}
          onPress={() => close(null)}
          style={({ pressed }) => [styles.cancel, pressed && styles.cancelPressed]}
        >
          <Text style={styles.cancelText}>{t('mobile.cancel')}</Text>
        </Pressable>
      </View>
    </Modal>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    root: { flex: 1, gap: 12, padding: 16, paddingTop: 48, backgroundColor: c.background },
    title: { fontSize: 20, fontWeight: '700', color: c.textPrimary },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    count: { fontSize: 15, color: c.textSecondary },
    list: { flex: 1 },
    listContent: { gap: 8, paddingBottom: 12 },
    row: {
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
    rowCurrent: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    rowRefused: { opacity: 0.6 },
    rowPressed: { backgroundColor: c.surfacePressed },
    marker: { fontSize: 18, width: 22, textAlign: 'center', color: c.textPrimary },
    rowText: { flex: 1, fontSize: 16, color: c.textPrimary },
    cancel: {
      alignSelf: 'flex-end',
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    cancelPressed: { backgroundColor: c.surfacePressed },
    cancelText: { fontSize: 16, fontWeight: '600', color: c.link },
  });
