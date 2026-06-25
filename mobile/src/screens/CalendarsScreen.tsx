import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import { isBirthdayCalendarId } from '@aperio/shared';
import type { ColorLabel } from '@aperio/shared';

import { listAccounts, type Account } from '../api/accounts';
import { Calendar, createCalendar, listCalendars } from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import type { RootStackScreenProps } from '../navigation/types';
import { refreshRemindersSoon } from '../reminders/scheduler';
import { useCacheReload } from '../state/cacheObserver';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Calendar catalog: read the calendars (local + external), create a local one,
// and open the editor to rename / recolour / delete a LOCAL calendar. External
// calendars are provider-managed (rename/colour are host-local overrides,
// deferred), so they're listed read-only — no Manage entry. Each row shows the
// calendar's bound colour as a real swatch for sighted users + the name on the
// accessible label.

const LOCAL = 'local';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function CalendarsScreen({
  navigation,
}: RootStackScreenProps<'Calendars'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  // Per-device calendar visibility (hide a calendar from every calendar view +
  // the event-target pickers).
  const { hidden, toggle: toggleVisibility } = useCalendarVisibility();

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [newName, setNewName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Resolve external calendars' owning account to its display name (not the raw
  // account id); local calendars read as "On this device".
  const [accountNames, setAccountNames] = useState<Map<string, string>>(new Map());

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );

  const load = useCallback(async () => {
    try {
      const [cals, labels, accounts] = await Promise.all([
        listCalendars(),
        listColorLabels().catch(() => [] as ColorLabel[]),
        listAccounts().catch(() => [] as Account[]),
      ]);
      setCalendars(cals);
      setColorLabels(labels);
      setAccountNames(new Map(accounts.map((a) => [a.id, a.display_name])));
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  // Reload on mount + whenever the screen regains focus (after the editor
  // renames / recolours / deletes a calendar).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update the catalog while focused when an external calendar-cache
  // refresh lands (the root observer already announced it politely).
  useCacheReload('calendar', load);

  // Move SR focus to the new row once the refreshed catalog re-renders.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [calendars]);

  const addCalendar = useCallback(async () => {
    if (busy) return;
    const name = newName.trim();
    if (name.length === 0) return;
    setError(null);
    setBusy(true);
    try {
      const created = await createCalendar({ name });
      setNewName('');
      await load();
      pendingFocusId.current = created.id;
      announce(t('sidebar.calendarCreated', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, load, newName, t]);

  return (
    <View style={styles.screen}>
      <View style={styles.form}>
        <TextInput
          style={styles.input}
          value={newName}
          onChangeText={setNewName}
          placeholder={t('sidebar.newCalendar')}
          accessibilityLabel={t('sidebar.newCalendar')}
          editable={!busy}
          returnKeyType="done"
          onSubmitEditing={() => void addCalendar()}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={t('sidebar.newCalendar')}
          disabled={busy}
          onPress={() => void addCalendar()}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('mobile.add')}</Text>
        </Pressable>
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {calendars.length === 0 ? (
        <Text style={styles.muted}>{t('mobile.noCalendars')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {calendars.map((cal) => {
            const isLocal = cal.account_id === LOCAL;
            const hex = cal.color_label
              ? labelsById.get(cal.color_label)?.hex
              : (cal.color?.hex ?? undefined);
            const colourName = cal.color_label
              ? labelsById.get(cal.color_label)?.name
              : undefined;
            // name + account + (colour name) on the accessible label.
            const accountLabel = isLocal
              ? t('sidebar.localAccount')
              : (accountNames.get(cal.account_id) ?? cal.account_id);
            const label =
              `${cal.name}, ${accountLabel}` +
              (colourName ? t('mobile.colorLabelSuffix', { name: colourName }) : '');
            return (
              <View key={cal.id} style={styles.row}>
                <Pressable
                  accessible
                  accessibilityRole="switch"
                  accessibilityState={{ checked: !hidden.has(cal.id) }}
                  accessibilityLabel={t('mobile.calendarVisible', { name: cal.name })}
                  onPress={() => {
                    toggleVisibility(cal.id);
                    // Re-filter scheduled OS notifications for the new visibility.
                    refreshRemindersSoon();
                  }}
                  style={({ pressed }) => [styles.visToggle, pressed && styles.pressed]}
                >
                  {/* Visual only — the Pressable owns the toggle + the switch
                      a11y trait (announced on toggle). */}
                  <View pointerEvents="none">
                    <Switch
                      value={!hidden.has(cal.id)}
                      trackColor={{ false: colors.border, true: colors.accent }}
                      importantForAccessibility="no"
                      accessibilityElementsHidden
                    />
                  </View>
                </Pressable>
                <View
                  ref={(node) => {
                    rowTags.current[cal.id] = node ? findNodeHandle(node) : null;
                  }}
                  accessible
                  accessibilityRole="text"
                  accessibilityLabel={label}
                  style={styles.rowText}
                >
                  {hex != null && (
                    <View
                      accessible={false}
                      importantForAccessibility="no"
                      style={[styles.colorDot, { backgroundColor: hex }]}
                    />
                  )}
                  <Text style={styles.calName} importantForAccessibility="no">
                    {cal.name}
                  </Text>
                  <Text style={styles.account} importantForAccessibility="no">
                    {accountLabel}
                  </Text>
                </View>
                {/* Manageable calendars: a local one carries its colour/name on
                    its row; an external one stores a host-local colour/name
                    override (delete stays local-only). Synthetic birthday
                    calendars are read-only with no backing row, so they offer no
                    Manage — rename/colour/delete wouldn't apply to them. */}
                {!isBirthdayCalendarId(cal.id) && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={`${t('mobile.manageCalendar')}: ${cal.name}`}
                    onPress={() =>
                      navigation.navigate('CalendarEditor', { calendarId: cal.id })
                    }
                    style={({ pressed }) => [styles.manageButton, pressed && styles.rowPressed]}
                  >
                    <Text style={styles.manageButtonText}>{t('mobile.manageCalendar')}</Text>
                  </Pressable>
                )}
              </View>
            );
          })}
        </ScrollView>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    form: { flexDirection: 'row', gap: 10, padding: 16, alignItems: 'center' },
    input: {
      flex: 1,
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    button: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    list: { gap: 12, padding: 16 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 6,
      paddingHorizontal: 10,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowText: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 10,
      paddingHorizontal: 6,
    },
    rowPressed: { backgroundColor: c.surfacePressed },
    pressed: { opacity: 0.7 },
    visToggle: { paddingVertical: 8, paddingHorizontal: 2 },
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    calName: { fontSize: 18, color: c.textPrimary },
    account: { fontSize: 13, color: c.textSecondary },
    manageButton: {
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    manageButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
