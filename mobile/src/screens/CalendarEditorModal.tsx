import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { ColorLabel } from '@aperio/shared';

import { Calendar, deleteCalendar, listCalendars } from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import { renameContainer, setContainerColorLabel } from '../api/containerColor';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import type { RootStackScreenProps } from '../navigation/types';

// Manage a single LOCAL calendar: rename it, bind a colour label, or delete it
// (its events cascade away). Mirrors ListEditorModal's interaction patterns
// (rename field + immediate-effect colour picker + confirmed delete), trimmed —
// calendars have no sections or parent. External calendars are provider-managed
// (rename/colour are host-local overrides, deferred), so this screen only ever
// opens for local calendars (CalendarsScreen gates the Manage entry).

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function CalendarEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'CalendarEditor'>) {
  const { calendarId } = route.params;
  const { t } = useTranslation();

  const [calendar, setCalendar] = useState<Calendar | null>(null);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [renameText, setRenameText] = useState('');
  const [colorLabel, setColorLabel] = useState('');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const reload = useCallback(async () => {
    const [cals, labels] = await Promise.all([
      listCalendars(),
      listColorLabels().catch(() => [] as ColorLabel[]),
    ]);
    setColorLabels(labels);
    const cal = cals.find((c) => c.id === calendarId) ?? null;
    setCalendar(cal);
    if (cal) {
      setRenameText(cal.name);
      setColorLabel(cal.color_label ?? '');
    }
    return cal;
  }, [calendarId]);

  useEffect(() => {
    void (async () => {
      try {
        await reload();
      } catch (err) {
        setError(errorMessage(err));
      } finally {
        setLoading(false);
      }
    })();
  }, [reload]);

  const isLocal = calendar?.account_id === 'local';

  const renameCalendar = useCallback(async () => {
    if (busy || calendar == null) return;
    const name = renameText.trim();
    if (name.length === 0 || name === calendar.name) return;
    setError(null);
    setBusy(true);
    try {
      await renameContainer(calendarId, 'calendar', name);
      await reload();
      announce(t('mobile.calendarRenamed', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, calendar, calendarId, reload, renameText, t]);

  const setColour = useCallback(
    async (colorLabelId: string) => {
      if (busy) return;
      setError(null);
      setBusy(true);
      try {
        await setContainerColorLabel(calendarId, 'calendar', colorLabelId || null);
        await reload();
        const name = calendar?.name ?? '';
        const colour = colorLabelId
          ? colorLabels.find((l) => l.id === colorLabelId)?.name
          : undefined;
        announce(
          colour != null
            ? t('sidebar.menu.colorSetAnnouncement', { name, color: colour })
            : t('sidebar.menu.colorClearedAnnouncement', { name }),
        );
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, calendar, calendarId, colorLabels, reload, t],
  );

  const removeCalendar = useCallback(() => {
    if (calendar == null || busy) return;
    const name = calendar.name;
    Alert.alert(
      t('sidebar.deleteCalendar', { name }),
      t('mobile.deleteCalendarMessage', { name }),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('mobile.delete'),
          style: 'destructive',
          onPress: () => {
            void (async () => {
              setError(null);
              setBusy(true);
              try {
                await deleteCalendar(calendarId);
                announce(t('sidebar.calendarDeleted', { name }));
                navigation.goBack();
              } catch (err) {
                const message = errorMessage(err);
                setError(message);
                announce(t('mobile.error', { message }));
                setBusy(false);
              }
            })();
          },
        },
      ],
    );
  }, [announce, busy, calendar, calendarId, navigation, t]);

  if (loading) {
    return (
      <View style={styles.screen}>
        <Text style={styles.muted} accessibilityLabel={t('views.loading')}>
          {t('views.loading')}
        </Text>
      </View>
    );
  }

  if (calendar == null) {
    // Deleted from another device while the modal was open.
    return (
      <View style={styles.screen}>
        <Text style={styles.muted} accessibilityRole="text">
          {t('views.agenda.empty')}
        </Text>
      </View>
    );
  }

  if (!isLocal) {
    // Defensive: CalendarsScreen only offers Manage for local calendars, but if
    // an external one is reached, it's provider-managed (rename/colour are
    // host-local overrides not yet on mobile) — show the name read-only.
    return (
      <ScrollView style={styles.screen} contentContainerStyle={styles.content}>
        <Text style={styles.title} accessibilityRole="header">
          {calendar.name}
        </Text>
      </ScrollView>
    );
  }

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <Text style={styles.title} accessibilityRole="header">
        {calendar.name}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {/* Rename */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('mobile.renameCalendarLabel')}
      </Text>
      <View style={styles.addRow}>
        <TextInput
          style={styles.input}
          value={renameText}
          onChangeText={setRenameText}
          accessibilityLabel={t('mobile.renameCalendarLabel')}
          editable={!busy}
          returnKeyType="done"
          onSubmitEditing={() => void renameCalendar()}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={t('mobile.rename')}
          disabled={busy}
          onPress={() => void renameCalendar()}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>{t('mobile.rename')}</Text>
        </Pressable>
      </View>

      {/* Colour */}
      <ColorLabelSelect
        value={colorLabel}
        labels={colorLabels}
        onChange={(id) => void setColour(id)}
        disabled={busy}
      />

      {/* Delete (its events cascade away) — confirmed. */}
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: busy }}
        accessibilityLabel={t('sidebar.deleteCalendar', { name: calendar.name })}
        disabled={busy}
        onPress={removeCalendar}
        style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
      >
        <Text style={styles.deleteButtonText}>
          {t('sidebar.deleteCalendar', { name: calendar.name })}
        </Text>
      </Pressable>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 16 },
  title: { fontSize: 22, fontWeight: '700', color: '#10131a' },
  heading: { fontSize: 17, fontWeight: '700', color: '#2b3240' },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  addRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
  input: {
    flex: 1,
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  addButton: {
    paddingVertical: 12,
    paddingHorizontal: 16,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
  },
  addButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  pressed: { opacity: 0.7 },
  deleteButton: {
    marginTop: 8,
    paddingVertical: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#f0c2bd',
    backgroundColor: '#fdecea',
    alignItems: 'center',
  },
  deleteButtonText: { fontSize: 16, fontWeight: '700', color: '#b42318' },
});
