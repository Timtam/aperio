import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { isBirthdayCalendarId, type ColorLabel } from '@aperio/shared';

import { ADAPTER_KIND_DEVICE_CALENDAR, listAccounts, type Account } from '../api/accounts';
import { Calendar, deleteCalendar, listCalendars } from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import { renameContainer, setContainerColorLabel } from '../api/containerColor';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { FormScrollView } from '../components/FormScrollView';
import { RemindersEditor } from '../components/RemindersEditor';
import { SoundSelect } from '../components/SoundSelect';
import type { RootStackScreenProps } from '../navigation/types';
import { SelectFieldButton } from '../components/SelectFieldButton';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import {
  bindSignature,
  signatureForCalendar,
  useSignatures,
} from '../state/useSignatures';
import { useSoundPref } from '../state/useSoundPref';
import { useThemedStyles, type ThemeColors } from '../theme';

// Manage a single calendar: rename it, bind a colour label, or (local only)
// delete it. Mirrors ListEditorModal's interaction patterns (rename field +
// immediate-effect colour picker + confirmed delete), trimmed — calendars have
// no sections or parent. A LOCAL calendar carries its name/colour on its own
// synced row; an EXTERNAL calendar's rename is pushed to its provider (falling
// back to a host-local name override) and its colour is a host-local override —
// both handled Rust-side, so this screen opens for every calendar (delete stays
// local-only, since an external calendar is provider-owned).

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function CalendarEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'CalendarEditor'>) {
  const { calendarId } = route.params;
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [calendar, setCalendar] = useState<Calendar | null>(null);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [renameText, setRenameText] = useState('');
  const [colorLabel, setColorLabel] = useState('');
  const [accountKind, setAccountKind] = useState<Account['adapter_kind'] | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // This calendar's default reminder sound (§14.4 container level), host-local +
  // inheritable. Offered for every calendar (local + external) since the sound
  // pref applies to any container's reminders.
  const sound = useSoundPref(`sound.calendar.${calendarId}`);
  // Per-calendar default reminders (§ iOS "Default Alert Times" parity): applied
  // at notification time to events in this calendar that carry no own reminder.
  // The Host's reminder computation already reads this pref, so it genuinely
  // fires. Offered for every calendar (the iCloud display-fill is its use case).
  const defaultReminders = useCalendarDefaultReminders(calendarId);
  const { signatures } = useSignatures();
  const [boundId, setBoundId] = useState('');
  useEffect(() => {
    let cancelled = false;
    void signatureForCalendar(calendarId, signatures).then((sig) => {
      if (!cancelled) setBoundId(sig?.id ?? '');
    });
    return () => {
      cancelled = true;
    };
  }, [calendarId, signatures]);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const reload = useCallback(async () => {
    const [cals, labels, accounts] = await Promise.all([
      listCalendars(),
      listColorLabels().catch(() => [] as ColorLabel[]),
      listAccounts().catch(() => [] as Account[]),
    ]);
    setColorLabels(labels);
    const cal = cals.find((c) => c.id === calendarId) ?? null;
    setCalendar(cal);
    // The owning account's kind decides whether a new appointment here can
    // carry reminders at all (the device calendar never writes them).
    setAccountKind(accounts.find((a) => a.id === cal?.account_id)?.adapter_kind ?? null);
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
  // Synthetic birthday calendars have no backing row: rename/colour/delete
  // don't apply, so the editor reduces to just their default reminders (+ the
  // container sound). Those DO fire — the Host's reminder scan reads this
  // calendar's `defaultReminders` and, being all-day, anchors them to the
  // day-carryover time (e.g. "one week before" → 7 days earlier at day-start).
  const isBirthday = isBirthdayCalendarId(calendarId);
  // Whether a default reminder here can be ATTACHED to new appointments, and
  // therefore whether the per-row "Applies" choice is offered: not a read-only
  // calendar (an iCal subscription), not a synthesized birthday calendar, and
  // not the device calendar, whose alarms are the OS's and are never written.
  const placementOffered =
    !isBirthday &&
    calendar != null &&
    !calendar.read_only &&
    accountKind !== ADAPTER_KIND_DEVICE_CALENDAR;

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
          {t('mobile.noCalendars')}
        </Text>
      </View>
    );
  }

  return (
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
      <Text style={styles.title} accessibilityRole="header">
        {calendar.name}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {/* A birthday calendar is synthesised + read-only: skip rename/colour
          (they'd no-op) and lead with a short hint that this screen just holds
          its default reminders. */}
      {isBirthday && (
        <Text style={styles.hint} accessibilityRole="text">
          {t('mobile.birthdayCalendarRemindersHint')}
        </Text>
      )}

      {/* Rename */}
      {!isBirthday && (
        <>
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
        </>
      )}

      {/* Default reminder sound (§14.4 container level) — System / Silent / use
          the global default. Host-local + inheritable. */}
      {!sound.loading && (
        <SoundSelect
          label={t('reminders.sound.label')}
          value={sound.value}
          allowInherit
          onChange={(next) => void sound.save(next)}
          disabled={busy}
        />
      )}

      {/* Per-calendar default reminders — applied to events without their own
          reminder (the iCloud "Default Alert Times" case). */}
      {/* The signature new appointments in this calendar carry. A binding is a
          default, not a restriction: every signature stays pickable in the
          editors regardless of which calendar the appointment is on. */}
      {signatures.length > 0 && (
        <SelectFieldButton<string>
          label={t('dialogs.signature.calendarLabel')}
          labelAsHeading
          value={boundId}
          options={[
            { value: '', label: t('dialogs.settings.signatures.bindingNone') },
            ...signatures.map((sig) => ({ value: sig.id, label: sig.name })),
          ]}
          onChange={(id) => {
            setBoundId(id);
            void bindSignature(calendarId, id || null);
          }}
        />
      )}

      {!defaultReminders.loading && (
        <View style={styles.field}>
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.settings.calendars.hint')}
          </Text>
          {/* Each entry also says where it lives (only in Aperio, or
              attached to new events) — only where a new appointment can
              carry reminders at all: not a read-only calendar (an iCal
              subscription), not a synthesized birthday calendar, not the
              device calendar (its alarms are the OS's, never written). */}
          <RemindersEditor
            mode="event"
            value={defaultReminders.value}
            onChange={defaultReminders.save}
            placement={placementOffered}
            // "On next app start" is host-local by construction and could
            // never ring as a calendar default — see the editor's prop.
            allowAppStart={false}
          />
          {/* What the per-row choice means — only where the control exists,
              so it never describes something the screen reader can't find. */}
          {placementOffered && (
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.calendars.hintPlacement')}
            </Text>
          )}
        </View>
      )}

      {/* Delete (its events cascade away) — local calendars only; an external
          calendar is provider-owned, so it can't be deleted from here. A local
          contact list's birthday calendar reports account_id 'local' too but is
          synthetic (no backing row), so exclude it — there's nothing to delete. */}
      {isLocal && !isBirthday && (
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
      )}
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    title: { fontSize: 22, fontWeight: '700', color: c.textPrimary },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel },
    field: { gap: 6 },
    hint: { fontSize: 13, color: c.textSecondary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    addRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
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
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      backgroundColor: c.accent,
    },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    pressed: { opacity: 0.7 },
    deleteButton: {
      marginTop: 8,
      paddingVertical: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
      alignItems: 'center',
    },
    deleteButtonText: { fontSize: 16, fontWeight: '700', color: c.danger },
  });
