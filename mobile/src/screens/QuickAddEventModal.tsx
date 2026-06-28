import { DateTimePicker } from '@expo/ui/community/datetime-picker';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { selectableEventCalendars } from '@aperio/shared';

import { createEvent, listCalendars, type Calendar } from '../api/calendar';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { useCancelHeader } from '../components/useCancelHeader';
import {
  formatLocalDate,
  formatLocalTime,
  parseLocalDate,
  parseLocalTime,
} from '../intl/dateTimeField';
import { useCalendarVisibility } from '../state/calendarVisibility';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// One-tap EVENT capture — the RN twin of the desktop QuickAddDialog. Minimal
// form (title + date + time + calendar); the event runs one hour from the
// chosen start. "More details …" hands the in-progress title/day/calendar to
// the full EventEditor. Mobile had no event quick-add before (events always
// went straight to the full editor); this brings event parity with the task
// quick-add and the day-activation create flow.

/** Local YYYY-MM-DD + HH:MM → RFC-3339 UTC, or null when unparseable. */
function localToIso(date: string, time: string): string | null {
  if (!date.trim()) return null;
  const d = new Date(`${date.trim()}T${time.trim() || '00:00'}`);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

export default function QuickAddEventModal({
  navigation,
  route,
}: RootStackScreenProps<'QuickAddEvent'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [title, setTitle] = useState('');
  // A tapped calendar day seeds the start day; otherwise today.
  const [date, setDate] = useState(
    () => route.params.anchor ?? formatLocalDate(new Date()),
  );
  const [time, setTime] = useState(() => formatLocalTime(new Date()));
  const [calId, setCalId] = useState(route.params.calendarId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const titleRef = useRef<TextInput | null>(null);

  // Cancel button in the header (first element) so the user can back out fast.
  useCancelHeader(navigation);

  // Load the calendars for the picker (best-effort; the route already passed a
  // sensible default calId so the form works before this resolves).
  useEffect(() => {
    void listCalendars()
      .then(setCalendars)
      .catch(() => setCalendars([]));
  }, []);

  // Drive SR focus + the keyboard into the title field on open, so a new event
  // is ready to type immediately (a modal must drive focus or VoiceOver lingers
  // on the trigger row).
  useEffect(() => {
    const tag = findNodeHandle(titleRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    titleRef.current?.focus();
  }, []);

  // Offer only writable, non-hidden calendars (plus the pre-selected one so it
  // always shows). Mirrors the full editor's calendar picker.
  const calendarOptions = useMemo(
    () =>
      selectableEventCalendars(calendars, {
        selectedIds: new Set(
          calendars.filter((c) => !hidden.has(c.id)).map((c) => c.id),
        ),
        currentId: calId,
      }).map((c) => ({ value: c.id, label: c.name })),
    [calendars, hidden, calId],
  );

  const fail = useCallback((message: string) => {
    setError(message);
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const onCreate = useCallback(async () => {
    const trimmed = title.trim();
    if (!trimmed) {
      fail(t('dialogs.event.titleRequired'));
      return;
    }
    if (!calId) {
      fail(t('dialogs.event.calendarRequired'));
      return;
    }
    const start = localToIso(date, time);
    if (!start) {
      fail(t('mobile.invalidDateTime'));
      return;
    }
    const end = new Date(
      new Date(start).getTime() + 60 * 60 * 1000,
    ).toISOString();
    setError(null);
    setSubmitting(true);
    try {
      await createEvent({
        calendar_id: calId,
        title: trimmed,
        description: null,
        location: null,
        start,
        end,
        all_day: false,
        recurrence: null,
        color_label: null,
        reminders: [],
        sound: null,
        attendees: [],
        send_invitations: false,
      });
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.event.created', { title: trimmed }),
      );
      navigation.goBack();
    } catch (err) {
      fail(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [calId, date, fail, navigation, t, time, title]);

  // Hand off to the full editor, carrying the title/day/calendar. Replace (not
  // push) so the quick-add doesn't linger behind the editor.
  const openFullEditor = useCallback(() => {
    navigation.replace('EventEditor', {
      eventId: null,
      calendarId: calId,
      anchor: date.trim() || undefined,
      initialTitle: title.trim() || undefined,
    });
  }, [calId, date, navigation, title]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      {error != null && (
        <Text
          style={styles.error}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {error}
        </Text>
      )}

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.event.fields.title')}</Text>
        <TextInput
          ref={titleRef}
          style={styles.input}
          value={title}
          onChangeText={setTitle}
          placeholder={t('dialogs.event.fields.title')}
          accessibilityLabel={t('dialogs.event.fields.title')}
          returnKeyType="done"
          onSubmitEditing={() => void onCreate()}
          autoComplete="off"
        />
      </View>

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.event.fields.startDate')}</Text>
        <DateTimePicker
          mode="date"
          display="compact"
          value={parseLocalDate(date)}
          onValueChange={(_, d) => setDate(formatLocalDate(d))}
          locale={i18n.language}
        />
      </View>

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.event.fields.startTime')}</Text>
        <DateTimePicker
          mode="time"
          display="compact"
          value={parseLocalTime(time)}
          onValueChange={(_, d) => setTime(formatLocalTime(d))}
          locale={i18n.language}
        />
      </View>

      {calendarOptions.length > 0 ? (
        <RadioGroup<string>
          label={t('dialogs.event.fields.calendar')}
          value={calId}
          options={calendarOptions}
          onChange={setCalId}
        />
      ) : (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.event.pickCalendar')}
        </Text>
      )}

      <View style={styles.buttons}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.create')}
          accessibilityState={{ disabled: submitting }}
          disabled={submitting}
          onPress={() => void onCreate()}
          style={({ pressed }) => [
            styles.button,
            pressed && styles.buttonPressed,
            submitting && styles.buttonDisabled,
          ]}
        >
          <Text style={styles.buttonText}>{t('dialogs.create')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.quickAdd.moreDetails')}
          onPress={openFullEditor}
          style={({ pressed }) => [
            styles.ghostButton,
            pressed && styles.ghostPressed,
          ]}
        >
          <Text style={styles.ghostButtonText}>
            {t('dialogs.quickAdd.moreDetails')}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.cancel')}
          onPress={() => navigation.goBack()}
          style={({ pressed }) => [
            styles.ghostButton,
            pressed && styles.ghostPressed,
          ]}
        >
          <Text style={styles.ghostButtonText}>{t('dialogs.cancel')}</Text>
        </Pressable>
      </View>
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 20, gap: 18 },
    field: { gap: 6 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    buttons: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, marginTop: 8 },
    button: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonDisabled: { opacity: 0.5 },
    buttonText: { fontSize: 17, fontWeight: '700', color: c.textOnAccent },
    ghostButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostPressed: { backgroundColor: c.surfacePressed },
    ghostButtonText: { fontSize: 17, fontWeight: '600', color: c.link },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
